use std::any::TypeId;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::app::AppHandle;
use crate::error::RuntimeMissing;
use crate::graph::scopes::input_reads;
use crate::key::{BindingKind, Key};
use crate::module::handle::ModuleRef;
use crate::module::meta::Meta;
use crate::runtime::Runtime;
use crate::timer::{BoxError, BoxFuture, Timer};
use crate::transport::Transport;
use crate::transport::controller::MountedHandler;
use crate::type_name::TypeName;

/// Implemented by a transport's server: handed to `app.bind(..)`, prepared and bound by
/// `listen()`, driven by `serve`, drained and closed by the shutdown sequence (§9.4, §9.5).
///
/// `listen()` calls [`prepare`](Self::prepare) on every server before it calls
/// [`bind`](Self::bind) on any (transports DESIGN §2.7, X6), so a configuration error is always
/// reported before a port conflict.
///
/// Anyone implementing `Server` receives the [`DrainToken`], a user-written transport included:
/// the rule is "only transports", not "only these crates".
pub trait Server: Send + Sync + 'static {
    type Transport: Transport;

    /// Builds everything that can fail without a socket: the route table and its duplicate
    /// checks, TLS loading and key/certificate matching, CORS validation, endpoint parsing, the
    /// check that inherited sockets exist, the backend's limits. Answers one error listing every
    /// failure this server found; `listen()` collects every server's into
    /// `StartupError::Configure`, redacted. Nothing may bind here.
    fn prepare(&mut self, mounted: Mounted<'_, Self::Transport>) -> impl Future<Output = Result<(), BoxError>> + Send {
        let _ = mounted;
        async { Ok(()) }
    }

    /// Acquires the sockets and mounts `mounted`'s handlers. All-or-nothing for this server's own
    /// listeners: one that fails closes every listener it opened before returning `Err`. An
    /// error fails `listen()` as `StartupError::Bind`, redacted, and `listen()` closes the
    /// servers already bound.
    fn bind(&mut self, mounted: Mounted<'_, Self::Transport>) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Accepts and serves until drained. Each call opens an `Execution`, at the root routed to
    /// its handler's module or in that module directly. An error before the drain starts the
    /// shutdown.
    fn serve(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Stop accepting, the protocol's way: HTTP/2 and gRPC send GOAWAY, HTTP/1 closes idle
    /// keep-alive connections, WebSocket closes idle connections with 1001 at once while a busy
    /// connection stops reading, finishes its messages and then closes with 1001. A connection's
    /// cleanup opens with `Execution::open_terminal(&token, ..)`; clone the token into each
    /// connection task.
    ///
    /// The drain window lasts until this future and every other server's have completed and no
    /// execution is live, and drops the future at the window's deadline. A server answering what
    /// arrives during the drain without opening an execution, a refusal, waits here for those
    /// answers to be sent, since the wait for live executions does not cover them.
    fn drain(&self, token: DrainToken) -> impl Future<Output = ()> + Send;

    /// Closes the sockets. An error is recorded as `ShutdownFailure::Close` with reason
    /// `Errored`, redacted, and a panic is recorded there as `Panicked`.
    ///
    /// Bounded by what is left of `shutdown_timeout`, or by `hook_timeout` when no cap is set.
    /// A `close` that exceeds its bound is dropped and recorded with reason `TimedOut`, and once
    /// the cap has expired, so is one that does not finish on its first poll.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// The addresses this server listens on once bound: port 0 reports the port the OS
    /// chose. `App<Bound>::addresses()` gathers every server's. A transport with no socket of its
    /// own, a broker link, reports none.
    fn bound(&self) -> Vec<BoundAddr> {
        Vec::new()
    }
}

/// What a server receives when it prepares and binds: its transport's handlers and the app.
pub struct Mounted<'a, T: Transport> {
    pub(crate) handlers: &'a [MountedHandler<T>],
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) runtime: Arc<dyn Runtime>,
}

impl<T: Transport> Mounted<'_, T> {
    pub fn handlers(&self) -> &[MountedHandler<T>] {
        self.handlers
    }

    /// The mounted handlers of another marker `U` (X20), for a server whose transport mounts
    /// handlers under two markers: `ulo_ws::GatewayTable::own_port` reads the message handlers
    /// through [`handlers`](Self::handlers) and the connect handlers `ulo-ws` mounts under
    /// `WsConnect` here, pairing them by controller. Empty when no handler of `U` mounted.
    pub fn handlers_of<U: Transport>(&self) -> Vec<MountedHandler<U>> {
        mounted_handlers::<U>(&self.app)
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// The app's `Timer`, which enforces the per-call deadlines a transport accepts: the
    /// [`runtime`](Self::runtime)'s clock.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.timer
    }

    /// The app's `Runtime`, which a transport spawns its connections and calls on. `listen()`
    /// refuses a transport on an app without one, so it is always present here.
    pub fn runtime(&self) -> &Arc<dyn Runtime> {
        &self.runtime
    }

    /// Every module's metadata of type `M`, with a handle to the module that wrote it, in
    /// collection order (transports DESIGN §3.3, X9).
    ///
    /// A transport resolves each entry through the module that declared it, inside the call's
    /// execution: `module.with_execution(&exec).get::<Mw>()`, so a per-execution middleware is
    /// built once per call and a binding only that module sees resolves as `wire()` checked it.
    pub fn module_meta<M: Meta>(&self) -> Vec<(ModuleRef, Arc<M>)> {
        let shared = &self.app.shared;
        let graph = shared.graph();
        graph
            .modules
            .iter()
            .filter_map(|module| {
                let value = Arc::clone(module.meta.get(&TypeId::of::<M>())?);
                let value = value.downcast::<M>().ok()?;
                Some((shared.module_ref(module.id), value))
            })
            .collect()
    }
}

impl<'a, T: Transport> Mounted<'a, T> {
    /// The handlers that read the execution input `I` without `Option`, each with the steps to
    /// the read: directly through a parameter, or through an execution-scoped service the handler
    /// reaches, as the wiring pass walks them for its input check (§6.4). A transport refuses in
    /// `prepare` an input its host cannot seed: the HTTP embedding refuses `ClientAddr` on a host
    /// that supplies no peer address.
    ///
    /// Empty for a type no transport declares as an input.
    pub fn handlers_reading<I: Send + Sync + 'static>(&self) -> Vec<InputReader<'a, T>> {
        let key = Key::of::<I, ()>();
        let graph = self.app.shared.graph();
        let mut readers = Vec::new();
        for record in &graph.handlers {
            let Some(handler) = self.handlers.iter().find(|handler| Arc::ptr_eq(&handler.info, &record.info)) else {
                continue;
            };
            for read in input_reads(&graph, record).into_iter().filter(|read| read.input == key) {
                readers.push(InputReader { handler, steps: read.steps });
            }
        }
        readers
    }
}

/// One handler's read of an execution input, as [`Mounted::handlers_reading`] reports it: the
/// handler and its transport, which a refusal writes as the path's first step against its
/// report's names, as in `UsersController::get (Http)`, then the steps after it.
pub struct InputReader<'a, T: Transport> {
    handler: &'a MountedHandler<T>,
    steps: Vec<String>,
}

impl<'a, T: Transport> InputReader<'a, T> {
    pub fn handler(&self) -> &'a MountedHandler<T> {
        self.handler
    }

    /// The handler's transport, `T`'s marker type.
    pub fn transport(&self) -> TypeName {
        TypeName::of::<T>()
    }

    /// The steps after the handler: each binding between it and the read, then the injection
    /// point, as in `Audit (execution)`, ``Dep<ClientAddr> (field `addr`)``.
    pub fn steps(&self) -> &[String] {
        &self.steps
    }
}

/// One address a server listens on, as [`Server::bound`] reports it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundAddr {
    /// The transport's key, `Transport::KEY`.
    pub transport: &'static str,
    pub addr: SocketAddr,
    pub tls: bool,
}

impl BoundAddr {
    pub fn new(transport: &'static str, addr: SocketAddr) -> Self {
        BoundAddr { transport, addr, tls: false }
    }

    pub fn tls(self, tls: bool) -> Self {
        BoundAddr { tls, ..self }
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
    fn transport_name(&self) -> TypeName;
    fn prepare<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>>;
    fn bind<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>>;
    fn serve(&self) -> BoxFuture<'_, Result<(), BoxError>>;
    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()>;
    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>>;
    fn bound(&self) -> Vec<BoundAddr>;
}

/// The graph's handlers for `T`, typed again, and the app's `Runtime`. `listen()` refuses a
/// transport on an app with no `Runtime` before preparing it; this answers the same refusal rather
/// than handing a server a runtime it does not have.
pub(crate) fn mounted_parts<T: Transport>(
    app: &AppHandle,
) -> Result<(Vec<MountedHandler<T>>, Arc<dyn Runtime>), RuntimeMissing> {
    let Some(runtime) = app.shared.config.runtime.clone() else {
        return Err(RuntimeMissing { transport: TypeName::of::<T>() });
    };
    Ok((mounted_handlers::<T>(app), runtime))
}

/// The graph's handlers for `T`, typed again, in mount order.
fn mounted_handlers<T: Transport>(app: &AppHandle) -> Vec<MountedHandler<T>> {
    let graph = app.shared.graph();
    graph
        .handlers
        .iter()
        .filter(|h| h.decl.transport == TypeName::of::<T>())
        .filter_map(|h| {
            let controller = graph.binding(h.controller).record.keys().next()?.name(BindingKind::Single);
            h.mounted(app.shared.module_ref(h.module), controller)
        })
        .collect()
}

impl<S: Server> ErasedServer for S {
    fn transport_name(&self) -> TypeName {
        TypeName::of::<S::Transport>()
    }

    fn prepare<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            let (handlers, runtime) = mounted_parts::<S::Transport>(app).map_err(BoxError::from)?;
            let timer = Arc::clone(&runtime) as Arc<dyn Timer>;
            <S as Server>::prepare(self, Mounted { handlers: &handlers, app: app.clone(), timer, runtime }).await
        })
    }

    fn bind<'a>(&'a mut self, app: &'a AppHandle) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            let (handlers, runtime) = mounted_parts::<S::Transport>(app).map_err(BoxError::from)?;
            let timer = Arc::clone(&runtime) as Arc<dyn Timer>;
            <S as Server>::bind(self, Mounted { handlers: &handlers, app: app.clone(), timer, runtime }).await
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

    fn bound(&self) -> Vec<BoundAddr> {
        Server::bound(self)
    }
}
