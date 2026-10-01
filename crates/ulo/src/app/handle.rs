use std::sync::Arc;

use crate::app::load;
use crate::app::shared::AppShared;
use crate::binding::Qualifier;
use crate::dependency::Dep;
use crate::error::{Closed, LoadError, LookupError, Shutdown, ShutdownError};
use crate::execution::notify::Draining;
use crate::execution::{ExecOptions, Execution};
use crate::lifecycle::shutdown;
use crate::module::Module;
use crate::module::handle::ModuleRef;
use crate::signal::Signal;

/// A `Clone + Send + Sync` view of the app's shared state, available from `Connected` on.
///
/// `serve(self)` consumes only the `Bound` typestate, so a handle taken before `serve` keeps
/// working while the app serves. A background loop holding one reads [`draining`](Self::draining)
/// to stop pulling work when the transports stop accepting, and runs each unit of work through
/// [`execute`](Self::execute) so the drain waits for it: the drain tracks executions, not tasks.
#[derive(Clone)]
pub struct AppHandle {
    pub(crate) shared: Arc<AppShared>,
}

impl AppHandle {
    /// `T` with the root module's visibility.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.shared.get_root::<T>().await
    }

    /// The one module of type `M`; see [`App::module`](crate::App).
    pub fn module<M: 'static>(&self) -> Result<ModuleRef, LookupError> {
        self.shared.find_module::<M>(None)
    }

    pub fn module_keyed<M: 'static, Q: 'static>(&self) -> Result<ModuleRef, LookupError> {
        self.shared.find_module::<M>(Some(Qualifier::of::<Q>()))
    }

    /// A standalone execution with the root module's visibility; see [`App::execute`](crate::App).
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        let root = self.shared.graph().root;
        self.shared.execute(root, opts, f).await
    }

    /// Wires `module` against the frozen graph, every error collected, then connects it through
    /// its own readiness checks and init hooks. Loading an equal identity twice returns the
    /// existing handle. Refused from Stopping on with `LoadError::Closed` (§8.6).
    pub async fn load(&self, module: impl Module) -> Result<ModuleRef, LoadError> {
        load::load(&self.shared, Box::new(module)).await
    }

    /// One of shutdown's two triggers; ends `serve`. A `close` during a running shutdown starts
    /// nothing: it waits for the same shutdown, ignores its own signal and returns the same
    /// outcome.
    pub async fn close(&self, signal: Signal) -> Result<Shutdown, ShutdownError> {
        shutdown::close(&self.shared, signal).await
    }

    /// The same notice an execution's `draining()` gives, resolving at the same moment.
    pub fn draining(&self) -> Draining<'_> {
        Draining::new(self.shared.draining.listen())
    }

    pub fn is_draining(&self) -> bool {
        self.shared.draining.is_fired()
    }
}
