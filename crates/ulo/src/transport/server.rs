use std::any::TypeId;
use std::future::Future;
use std::sync::Arc;

use crate::app::AppHandle;
use crate::error::NoTimer;
use crate::key::BindingKind;
use crate::timer::{BoxError, BoxFuture, Timer};
use crate::transport::controller::MountedHandler;
use crate::transport::{Transport, transport_name};

/// Implemented by a transport's server: handed to `app.bind(..)`, bound by `listen()`, driven by
/// `serve`, drained and closed by the shutdown sequence (§9.4, §9.5).
///
/// Anyone implementing `Server` receives the [`DrainToken`], a user-written transport included:
/// the rule is "only transports", not "only these crates".
pub trait Server: Send + Sync + 'static {
    type Transport: Transport;

    /// Acquires the sockets and mounts `mounted`'s handlers. An error fails `listen()` as
    /// `StartupError::Bind`, redacted.
    fn bind(&mut self, mounted: Mounted<'_, Self::Transport>) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Accepts and serves until drained. Each call opens an `Execution` in its handler's module.
    /// An error before the drain starts the shutdown.
    fn serve(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Stop accepting, the protocol's way: HTTP/2 and gRPC send GOAWAY, HTTP/1 closes idle
    /// keep-alive connections, WebSocket closes idle connections with 1001 at once while a busy
    /// connection stops reading, finishes its messages and then closes with 1001. A connection's
    /// cleanup opens with `Execution::open_terminal(&token, ..)`; clone the token into each
    /// connection task.
    fn drain(&self, token: DrainToken) -> impl Future<Output = ()> + Send;

    /// Closes the sockets. An error is recorded as `ShutdownFailure::Close` with reason
    /// `Errored`, redacted, and a panic is recorded there as `Panicked`.
    ///
    /// Bounded by what is left of `shutdown_timeout`, or by `hook_timeout` when no cap is set.
    /// A `close` that exceeds its bound is dropped and recorded with reason `TimedOut`, and once
    /// the cap has expired, so is one that does not finish on its first poll.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

/// What a server receives when it binds: its transport's handlers and the app.
pub struct Mounted<'a, T: Transport> {
    pub(crate) handlers: &'a [MountedHandler<T>],
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
}

impl<T: Transport> Mounted<'_, T> {
    pub fn handlers(&self) -> &[MountedHandler<T>] {
        self.handlers
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// The app's `Timer`, which enforces the per-call deadlines a transport accepts. `listen()`
    /// refuses a transport on an app without one, so it is always present here.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.timer
    }
}

/// The proof that a transport opens a terminal execution. Private field, one source: the core
/// hands it to each transport through `Server::drain` as Draining begins. `Clone`, an `Arc`
/// inside, because a transport opens terminal executions from connection tasks spawned long
/// before the drain; cloning creates no second source.
#[derive(Clone)]
pub struct DrainToken {
    _inner: Arc<()>,
}

impl DrainToken {
    pub(crate) fn new() -> Self {
        DrainToken { _inner: Arc::new(()) }
    }
}

/// The dyn-compatible twin of `Server` the app stores, one per bound transport.
pub(crate) trait ErasedServer: Send + Sync + 'static {
    fn transport_name(&self) -> &'static str;
    fn bind<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>>;
    fn serve(&self) -> BoxFuture<'_, Result<(), BoxError>>;
    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()>;
    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>>;
}

impl<S: Server> ErasedServer for S {
    fn transport_name(&self) -> &'static str {
        transport_name::<S::Transport>()
    }

    /// Collects the graph's handlers for `S::Transport` into `MountedHandler`s and calls
    /// `Server::bind` with them.
    fn bind<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            // `listen()` refuses a transport on an app with no `Timer` before binding it; this
            // answers the same refusal rather than handing a server a clock it does not have.
            let Some(timer) = app.shared.config.timer.clone() else {
                return Err(BoxError::from(NoTimer { transport: transport_name::<S::Transport>() }));
            };
            let handlers: Vec<MountedHandler<S::Transport>> = {
                let graph = app.shared.graph();
                graph
                    .handlers
                    .iter()
                    .filter(|h| h.decl.transport == TypeId::of::<S::Transport>())
                    .filter_map(|h| {
                        let controller = graph.binding(h.controller).record.keys().next()?.name(BindingKind::Single);
                        h.mounted(app.shared.module_ref(h.module), controller)
                    })
                    .collect()
            };
            <S as Server>::bind(self, Mounted { handlers: &handlers, app: app.clone(), timer }).await
        })
    }

    fn serve(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(Server::serve(self))
    }

    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        Box::pin(Server::drain(self, token))
    }

    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(Server::close(self))
    }
}
