use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::error::{Closed, LoadError, LookupError, Shutdown, ShutdownError};
use crate::execution::notify::Draining;
use crate::execution::{ExecOptions, Execution};
use crate::module::Module;
use crate::module::handle::ModuleRef;
use crate::signal::Signal;
use crate::site::Dep;

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
        todo!()
    }

    pub fn module<M: 'static>(&self) -> Result<ModuleRef, LookupError> {
        todo!()
    }

    pub fn module_keyed<M: 'static, Q: 'static>(&self) -> Result<ModuleRef, LookupError> {
        todo!()
    }

    /// A standalone execution with the root module's visibility; see [`App::execute`](crate::App).
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        todo!()
    }

    /// Wires `module` against the frozen graph, every error collected, then connects it through
    /// its own readiness checks and init hooks. Loading an equal identity twice returns the
    /// existing handle. Refused from Stopping on with `LoadError::Closed` (§8.6).
    pub async fn load(&self, module: impl Module) -> Result<ModuleRef, LoadError> {
        todo!()
    }

    /// One of shutdown's two triggers; ends `serve`. A `close` during a running shutdown starts
    /// nothing: it waits for the same shutdown, ignores its own signal and returns the same
    /// outcome.
    pub async fn close(&self, signal: Signal) -> Result<Shutdown, ShutdownError> {
        todo!()
    }

    /// The same notice an execution's `draining()` gives, resolving at the same moment.
    pub fn draining(&self) -> Draining<'_> {
        todo!()
    }

    pub fn is_draining(&self) -> bool {
        todo!()
    }
}
