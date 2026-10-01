//! The application: the graph plus the singleton store, moving through typestates
//! `Wired` → `Connected` → `Bound` → serving → closed (§2, §9.4).
//!
//! From `Connected` on it hands out an [`AppHandle`], a `Clone + Send + Sync` view of the shared
//! state that outlives `serve`. The application value is `Send`.
//!
//! [`Bound`] here is the typestate after `listen()`; the timeout enum is [`crate::Bound`].

pub(crate) mod handle;
pub(crate) mod load;
pub(crate) mod shared;

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use crate::app::shared::AppShared;
use crate::error::{Closed, LookupError, Shutdown, ShutdownError, StartupError};
use crate::execution::{ExecOptions, Execution};
use crate::module::Module;
use crate::module::handle::ModuleRef;
use crate::signal::Signal;
use crate::site::Dep;
use crate::testing::TestPlan;
use crate::timer::{Defaults, Timer};
use crate::transport::server::{ErasedServer, Server};

pub use handle::AppHandle;

/// Wired: the graph is built and every wiring error has been reported. Nothing is built yet.
pub enum Wired {}

/// Connected: singletons built, readiness checks passed, init and bootstrap hooks run.
pub enum Connected {}

/// Bound: every transport holds its sockets. `serve` consumes this state.
pub enum Bound {}

pub struct App<S = Wired> {
    pub(crate) shared: Arc<AppShared>,
    /// Queued by `bind` on `Connected`, bound by `listen`.
    pub(crate) servers: Vec<Box<dyn ErasedServer>>,
    _s: PhantomData<S>,
}

/// Configures an app before `wire()`: the root module, the `Timer` and the four timing knobs.
/// Every knob is timed by the `Timer`; set on a builder with none, each is a wiring error.
pub struct AppBuilder {
    pub(crate) root: Box<dyn Module>,
    pub(crate) config: AppConfig,
    pub(crate) plan: Option<TestPlan>,
}

/// The app's timing configuration. `None` is unset.
#[derive(Clone, Default)]
pub(crate) struct AppConfig {
    pub(crate) timer: Option<Arc<dyn Timer>>,
    pub(crate) drain_timeout: Option<Duration>,
    pub(crate) shutdown_timeout: Option<Duration>,
    pub(crate) hook_timeout: Option<Duration>,
    pub(crate) construct_timeout: Option<Duration>,
}

impl AppConfig {
    pub(crate) const DEFAULT_DRAIN: Duration = Duration::from_secs(10);
    pub(crate) const DEFAULT_HOOK: Duration = Duration::from_secs(10);
    pub(crate) const DEFAULT_CONSTRUCT: Duration = Duration::from_secs(30);

    /// The defaults `Bound::Default` resolves against; `None` without a `Timer`, where an item
    /// left at `Default` runs unbounded.
    pub(crate) fn defaults(&self) -> Option<Defaults> {
        todo!()
    }

    /// The drain window: zero without a `Timer`, which abandons live executions at once.
    pub(crate) fn drain(&self) -> Duration {
        todo!()
    }

    /// The names of the knobs that were set, for the environment check.
    pub(crate) fn knobs_set(&self) -> Vec<&'static str> {
        todo!()
    }
}

impl App {
    pub fn builder(root: impl Module) -> AppBuilder {
        AppBuilder { root: Box::new(root), config: AppConfig::default(), plan: None }
    }
}

impl AppBuilder {
    /// The app's one clock; bound for services as `Dep<dyn Timer>` in the core's global module.
    pub fn timer(mut self, timer: impl Timer) -> Self {
        self.config.timer = Some(Arc::new(timer));
        self
    }

    /// In-flight executions at shutdown; ten seconds unset.
    pub fn drain_timeout(mut self, d: Duration) -> Self {
        self.config.drain_timeout = Some(d);
        self
    }

    /// The whole close sequence, from the trigger; unset by default. Set it a few seconds under
    /// the orchestrator's grace period.
    pub fn shutdown_timeout(mut self, d: Duration) -> Self {
        self.config.shutdown_timeout = Some(d);
        self
    }

    /// The `Default` every hook starts with; ten seconds unset.
    pub fn hook_timeout(mut self, d: Duration) -> Self {
        self.config.hook_timeout = Some(d);
        self
    }

    /// The `Default` every construction and one attempt of every readiness check starts with;
    /// thirty seconds unset.
    pub fn construct_timeout(mut self, d: Duration) -> Self {
        self.config.construct_timeout = Some(d);
        self
    }

    /// Builds the graph and validates everything, collecting every error in one pass. No
    /// instances, no I/O.
    pub fn wire(self) -> Result<App<Wired>, StartupError> {
        todo!()
    }
}

impl<S> App<S> {
    pub(crate) fn from_shared(shared: Arc<AppShared>, servers: Vec<Box<dyn ErasedServer>>) -> Self {
        App { shared, servers, _s: PhantomData }
    }
}

impl App<Wired> {
    /// Builds the singletons in dependency order, running each one's readiness check right after
    /// it, then every `OnModuleInit` hook, then every `OnApplicationBootstrap` hook (§9.2).
    pub async fn connect(self) -> Result<App<Connected>, StartupError> {
        todo!()
    }
}

impl App<Connected> {
    /// Queues a transport; `listen` binds every queued transport.
    pub fn bind(mut self, server: impl Server) -> App<Connected> {
        todo!()
    }

    /// Mounts every handler on its transport and acquires the sockets. Refuses an app that binds
    /// a transport with no `Timer`: `StartupError::Bind` carrying `NoTimer` in its `source`.
    pub async fn listen(self) -> Result<App<Bound>, StartupError> {
        todo!()
    }

    pub fn handle(&self) -> AppHandle {
        AppHandle { shared: Arc::clone(&self.shared) }
    }

    /// `T` with the root module's visibility.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        todo!()
    }

    /// The one module of type `M`: `LookupError::AmbiguousModule` if several configurations of
    /// it are registered. `M` is the type a module's identity names, the owner type for a
    /// `DynamicModule`.
    pub fn module<M: 'static>(&self) -> Result<ModuleRef, LookupError> {
        todo!()
    }

    /// The keyed instance `Q` of `M`.
    pub fn module_keyed<M: 'static, Q: 'static>(&self) -> Result<ModuleRef, LookupError> {
        todo!()
    }

    /// A standalone execution with the root module's visibility (§6.3). Refused from Draining on
    /// with `Closed`. `execute` drops the execution when the closure's future completes, which
    /// fires no cancellation; a subtask that outlives it takes `exec.handle()`.
    ///
    /// An inherent `async fn`, so its future is `Send` whenever the closure's future is.
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        todo!()
    }

    /// Runs the shutdown sequence with no sockets to close: for a job, a CLI command or a test
    /// that stops after `connect()`.
    pub async fn close(self, signal: Signal) -> Result<Shutdown, ShutdownError> {
        todo!()
    }
}

impl App<Bound> {
    pub fn handle(&self) -> AppHandle {
        AppHandle { shared: Arc::clone(&self.shared) }
    }

    /// Serves until `signal` resolves or a `close` on any handle, whichever comes first, then
    /// runs the shutdown sequence and returns its outcome (§9.5).
    pub async fn serve(self, signal: impl Future<Output = Signal> + Send) -> Result<Shutdown, ShutdownError> {
        todo!()
    }
}
