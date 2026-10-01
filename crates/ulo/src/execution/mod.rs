//! Executions: one call, or one standalone job (§3.8, §6.3).
//!
//! The transport holds the [`Execution`] for a call and `execute` holds it for a standalone one;
//! everything else holds an [`ExecutionRef`], directly or inside a transport's `Cx`. Three
//! events are distinct:
//! - **Draining** is the notice: it resolves when the app stops accepting, after the
//!   before-shutdown stage.
//! - **Cancellation** fires while handles are alive: for a call when the client disconnects or
//!   its deadline passes, for a standalone execution at its deadline, and for either at the end
//!   of the drain. Completion of `execute` does not fire it.
//! - **End of execution** is when the last handle drops: the per-execution cache is released
//!   and execution-scoped instances drop then, so a streaming reply keeps its instances exactly
//!   as long as it runs.

pub(crate) mod cache;
pub(crate) mod extensions;
pub(crate) mod notify;

use std::sync::Arc;
use std::time::Instant;

use crate::app::shared::{AppShared, LiveSlot};
use crate::error::{Closed, LookupError};
use crate::execution::cache::ExecCache;
use crate::execution::extensions::{Extensions, Inputs};
use crate::execution::notify::{Cancelled, Draining, Notify};
use crate::graph::ModuleId;
use crate::module::handle::ModuleRef;
use crate::resolver::{Purpose, Resolver};
use crate::site::Dep;
use crate::transport::server::DrainToken;

/// One execution, as its holder owns it. Not `Clone`: a second holder takes [`handle`](Self::handle).
///
/// `Execution: Send + Sync`, and so is the `Resolver` read through it, which is what lets every
/// site's future be `Send`.
pub struct Execution {
    pub(crate) shared: Arc<ExecShared>,
}

/// A cheap-clone handle to an execution: the same methods as [`Execution`] except `seed`, so an
/// input a guard or a subtask seeded could never be one the wiring pass did not see.
#[derive(Clone)]
pub struct ExecutionRef {
    pub(crate) shared: Arc<ExecShared>,
}

/// Options for a new execution.
#[derive(Clone, Debug, Default)]
pub struct ExecOptions {
    pub(crate) deadline: Option<Instant>,
}

impl ExecOptions {
    pub fn new() -> Self {
        ExecOptions::default()
    }

    /// Cancellation fires when `at` passes, read on the app's `Timer`. The core stores the
    /// deadline; for a call the transport's runtime enforces it.
    pub fn deadline(self, at: Instant) -> Self {
        ExecOptions { deadline: Some(at) }
    }
}

/// The state every handle to one execution shares.
pub(crate) struct ExecShared {
    pub(crate) app: Arc<AppShared>,
    /// The visibility the execution resolves with: the dispatching controller's module, the root
    /// for `App::execute` and `AppHandle::execute`, `M` for `ModuleRef::execute`.
    pub(crate) module: ModuleId,
    pub(crate) cache: ExecCache,
    pub(crate) extensions: Extensions,
    pub(crate) inputs: Inputs,
    pub(crate) cancel: Notify,
    pub(crate) deadline: Option<Instant>,
    /// Opened with `open_terminal`; counted in `Shutdown::terminal_skipped` when abandoned.
    pub(crate) terminal: bool,
    /// The execution's entry in the app's live set; dropping it is the end of the execution the
    /// drain waits for.
    pub(crate) live: LiveSlot,
}

impl Execution {
    /// Opens an execution resolving with `module`'s visibility. Refused from Draining on with
    /// `Closed`. For transports and standalone code alike.
    pub fn open(module: &ModuleRef, opts: ExecOptions) -> Result<Execution, Closed> {
        module.app.open(module.module, opts, false)
    }

    /// Opens a connection's cleanup during the drain: allowed in Draining, counted in the drain
    /// and bounded by `drain_timeout` like any other execution. The token proves a transport
    /// opens it; the phase check decides whether it may, so a token used after the drain has
    /// ended gets `Closed`.
    pub fn open_terminal(token: &DrainToken, module: &ModuleRef, opts: ExecOptions) -> Result<Execution, Closed> {
        let _ = token;
        module.app.open(module.module, opts, true)
    }

    /// Seeds an execution input. Inputs are `Send + Sync`, like everything the execution holds.
    pub fn seed<T: Send + Sync + 'static>(&self, input: T) {
        self.shared.inputs.insert(input);
    }

    /// An owned clone, for a spawned subtask; it keeps the cache and the execution-scoped
    /// instances alive until it drops.
    pub fn handle(&self) -> ExecutionRef {
        ExecutionRef { shared: Arc::clone(&self.shared) }
    }

    pub fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    pub fn cancelled(&self) -> Cancelled<'_> {
        self.shared.cancelled()
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_fired()
    }

    /// Resolves when the app stops accepting (§9.5); false through the before-shutdown stage.
    pub fn draining(&self) -> Draining<'_> {
        self.shared.draining()
    }

    pub fn is_draining(&self) -> bool {
        self.shared.is_draining()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.shared.deadline
    }

    /// Fires cancellation: the transport calls it when the client disconnects or the deadline
    /// passes. Idempotent.
    pub fn cancel(&self) {
        self.shared.cancel.fire();
    }

    /// `T` with this execution's module visibility, built in this execution if it is
    /// execution-scoped.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.resolver().dep::<T>().await
    }

    /// The resolver a transport reads through, for `Resolver::entries` and `Site::read`.
    pub fn resolver(&self) -> Resolver<'_> {
        self.shared.resolver()
    }
}

impl ExecutionRef {
    pub fn handle(&self) -> ExecutionRef {
        self.clone()
    }

    pub fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    pub fn cancelled(&self) -> Cancelled<'_> {
        self.shared.cancelled()
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_fired()
    }

    pub fn draining(&self) -> Draining<'_> {
        self.shared.draining()
    }

    pub fn is_draining(&self) -> bool {
        self.shared.is_draining()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.shared.deadline
    }

    pub fn cancel(&self) {
        self.shared.cancel.fire();
    }

    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.resolver().dep::<T>().await
    }

    pub fn resolver(&self) -> Resolver<'_> {
        self.shared.resolver()
    }
}

impl ExecShared {
    pub(crate) fn resolver(self: &Arc<Self>) -> Resolver<'_> {
        Resolver::new(&self.app, self.module, Some(self), Purpose::Lookup)
    }

    pub(crate) fn cancelled(&self) -> Cancelled<'_> {
        Cancelled::new(self.cancel.listen())
    }

    pub(crate) fn draining(&self) -> Draining<'_> {
        Draining::new(self.app.draining.listen())
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.app.draining.is_fired()
    }
}
