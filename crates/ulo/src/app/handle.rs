use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::app::load;
use crate::app::shared::AppShared;
use crate::binding::Qualifier;
use crate::dependency::Dep;
use crate::error::{Closed, DispatchStage, LoadError, LookupError, Shutdown, ShutdownError, TimerMissing};
use crate::execution::notify::Draining;
use crate::execution::{ExecOptions, Execution};
use crate::lifecycle::phase::Phase as Stage;
use crate::lifecycle::shutdown;
use crate::module::Module;
use crate::module::handle::ModuleRef;
use crate::redact::{Redacted, redact};
use crate::signal::Signal;
use crate::timer::BoxError;
use crate::transport::Transport;
use crate::transport::controller::MountedHandler;
use crate::transport::handler::HandlerInfo;
use crate::transport::pipeline::caught;
use crate::transport::server::mounted_parts;

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

    /// The drain window `AppBuilder::drain_timeout` set, ten seconds unset; zero on an app without
    /// a `Timer`, which abandons live executions at once. A transport whose host server drains on
    /// its own clock passes this to it, so the window is set in one place.
    pub fn drain_timeout(&self) -> Duration {
        self.shared.config.drain()
    }

    /// Where the app is in its life (transports DESIGN §11, X13). gRPC health reads it to report
    /// NOT_SERVING from the drain on, consistent with `is_draining()`.
    pub fn phase(&self) -> Phase {
        match self.shared.phase.get() {
            Stage::Wired | Stage::Connecting | Stage::Running => Phase::Running,
            Stage::Stopping => Phase::Stopping,
            Stage::Draining => Phase::Draining,
            Stage::Destroying => Phase::Destroying,
            Stage::Closed => Phase::Closed,
        }
    }

    /// Runs the graph's own redaction over `err`: every `Secret` registered with this app is
    /// replaced, and userinfo is stripped from anything shaped like a URL (§9.3, X14). For code
    /// outside the core that stores an outside error where it may be printed, such as
    /// `ExtractError::Malformed`'s decoder error.
    pub fn redact(&self, err: BoxError) -> Redacted {
        redact(&self.shared.graph().secrets, err)
    }

    /// Every mounted handler, of every transport, with its transport key, route or pattern and
    /// metadata, in the order the controllers mounted them (transports DESIGN §2.5, X3).
    pub fn handlers(&self) -> Vec<Arc<HandlerInfo>> {
        self.shared.graph().handlers.iter().map(|handler| Arc::clone(&handler.info)).collect()
    }

    /// The mounted handlers of transport `T`, with their modules, enhancer tiers and handler
    /// values, as a `Server<Transport = T>` receives them through `Mounted` (X20). For code holding
    /// an `AppHandle` alone: the upgrade handler on the HTTP server's port finds a same-port
    /// gateway's handlers through it.
    ///
    /// Refused as `listen()` refuses a transport on an app with no `Timer`, since a handler's call
    /// needs the clock its deadlines run on.
    pub fn mounted<T: Transport>(&self) -> Result<Vec<MountedHandler<T>>, TimerMissing> {
        mounted_parts::<T>(self).map(|(handlers, _)| handlers)
    }

    /// The root module, whose visibility a lookup naming no module uses. A transport opens an
    /// execution here when the route is not known yet, and routes it with `Execution::route_to`.
    pub fn root(&self) -> ModuleRef {
        self.shared.root()
    }

    /// Polls `fut` and turns a panic in it into `PanicRecovered { stage, .. }`, its message
    /// redacted, as `dispatch` does for its own stages. For a transport's pre-dispatch stage, whose
    /// entries run outside `dispatch` and still reach the error handlers on a panic.
    pub async fn catch_panic<R, F>(&self, stage: DispatchStage, fut: F) -> Result<R, BoxError>
    where
        F: Future<Output = Result<R, BoxError>>,
    {
        let graph = self.shared.graph();
        caught(stage, &graph.secrets, fut).await
    }
}

/// The app's phase as [`AppHandle::phase`] reports it (§9.5). An `AppHandle` exists from
/// `Connected` on, so the phases before it read as `Running`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// Serving, or connected and not yet serving.
    Running,
    /// The before-shutdown stage: traffic is normal and `is_draining()` is false.
    Stopping,
    /// From stop-accepting to the drain's end: new executions are refused.
    Draining,
    /// From the first destroy hook on: singleton lookups answer `LookupError::Closed`.
    Destroying,
    Closed,
}
