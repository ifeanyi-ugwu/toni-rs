use crate::dispatch::Items;
use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::error::{ResolutionError, StartupError};
use event_listener::Event;

use crate::{
    application_context::UloApplicationContext,
    di::internal::{Container, IntoToken},
    dispatch::resolve::GatewayResolver,
    grpc::GrpcAdapter,
    http::RouteMount,
    http::{HttpAdapter, ServeContext},
    rpc::{RpcAdapter, RpcCallInfo, RpcControllerWrapper, RpcData, RpcError, RpcMessageCallbacks},
    server_lifecycle::ServerLifecycle,
    spi::BindTarget,
    ws::{
        BroadcastService, DisconnectReason, GatewayWrapper, MessageCallbackResult, WsAdapter,
        WsClientMap, WsConnectionCallbacks, helpers::create_client_from_parts,
    },
};

struct ShutdownInner {
    shutdown_flag: AtomicBool,
    shutdown_event: Event,
    completed_flag: AtomicBool,
    completed_event: Event,
}

/// A cloneable handle for signalling and observing application shutdown.
///
/// Created by [`UloApplication::shutdown_handle`] and usable from any task.
/// Calling [`shutdown`](ShutdownHandle::shutdown) is idempotent — multiple
/// callers are safe.
#[derive(Clone)]
pub struct ShutdownHandle {
    inner: Arc<ShutdownInner>,
}

/// Prints the two flags a caller can observe.
///
/// Written rather than derived: a derive would need one on the private `ShutdownInner` too, and
/// would render its two `event_listener::Event` wakers, which carry no state worth reading.
/// `completed` is taken from the flag because [`ShutdownHandle::completed`] awaits rather than
/// reports.
impl std::fmt::Debug for ShutdownHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownHandle")
            .field("is_shutdown", &self.is_shutdown())
            .field(
                "completed",
                &self.inner.completed_flag.load(Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

impl ShutdownHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(ShutdownInner {
                shutdown_flag: AtomicBool::new(false),
                shutdown_event: Event::new(),
                completed_flag: AtomicBool::new(false),
                completed_event: Event::new(),
            }),
        }
    }

    /// Signal the application to shut down. Safe to call from multiple tasks;
    /// only the first call takes effect.
    pub fn shutdown(&self) {
        if !self.inner.shutdown_flag.swap(true, Ordering::SeqCst) {
            self.inner.shutdown_event.notify(usize::MAX);
        }
    }

    /// Returns `true` if shutdown has been signalled.
    pub fn is_shutdown(&self) -> bool {
        self.inner.shutdown_flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`shutdown`](ShutdownHandle::shutdown) has been called.
    pub async fn wait_for_shutdown(&self) {
        loop {
            if self.inner.shutdown_flag.load(Ordering::SeqCst) {
                return;
            }
            let listener = self.inner.shutdown_event.listen();
            if self.inner.shutdown_flag.load(Ordering::SeqCst) {
                return;
            }
            listener.await;
        }
    }

    /// Resolves once [`run`](UloApplication::run) has returned — i.e. all
    /// adapters are closed and lifecycle hooks have completed.
    pub async fn completed(&self) {
        loop {
            if self.inner.completed_flag.load(Ordering::SeqCst) {
                return;
            }
            let listener = self.inner.completed_event.listen();
            if self.inner.completed_flag.load(Ordering::SeqCst) {
                return;
            }
            listener.await;
        }
    }

    fn mark_completed(&self) {
        self.inner.completed_flag.store(true, Ordering::SeqCst);
        self.inner.completed_event.notify(usize::MAX);
    }
}

/// The addresses of all bound adapters, returned by [`UloApplication::bind`].
#[derive(Debug)]
pub struct BoundAdapters {
    /// The address the HTTP adapter is listening on, or `None` if no HTTP
    /// adapter was registered.
    pub http: Option<SocketAddr>,
    /// One address per unique separate-port WebSocket listener that was bound.
    pub websocket: Vec<SocketAddr>,
    /// The address the RPC adapter is listening on, when it binds to one.
    /// `None` for subject-based transports (NATS, Kafka) that have no local
    /// listener, or when no RPC adapter was registered — never because one
    /// failed to start, which fails [`bind`](UloApplication::bind) instead.
    pub rpc: Option<SocketAddr>,
    /// The address the gRPC adapter is listening on, or `None` if no gRPC
    /// adapter was registered.
    pub grpc: Option<SocketAddr>,
}

#[derive(Debug, PartialEq)]
enum AppState {
    Configuring,
    Bound,
    /// A `bind()` that returned an error. Adapters were consumed and their
    /// sockets released on the way out, so the application cannot be bound
    /// again — build a new one.
    Failed,
}

struct BoundState {
    serve_futures: Vec<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

/// An application built from a module graph, holding the adapters it serves on.
///
/// Startup runs in three phases, each with its own failure class:
///
/// - [`UloFactory::create`](crate::UloFactory::create) builds the graph, constructs its
///   providers and runs their `on_module_init` hooks — where an integration confirms the
///   **outbound** dependencies it needs will answer. Fails when the graph does not resolve, or a
///   dependency is unreachable.
/// - [`bind`](Self::bind) runs `on_application_bootstrap`, then takes the **inbound** sockets this
///   process serves on. Fails when a declared transport has no adapter to serve it, or the OS
///   refuses an address.
/// - [`run`](Self::run) drives the serve loops until shutdown.
///
/// Keeping the last two apart is what lets a caller read [`BoundAdapters`] before anything is
/// served: the address the OS assigned for port 0, a readiness gate opened once the sockets are
/// live, a socket handed to a replacement process. Fused, a bound address would be unobservable
/// until the process was already serving.
pub struct UloApplication {
    // Adapters live here in `Configuring` state, before `bind()` consumes
    // them into lifecycle handles. After `bind()` succeeds these are all
    // `None` and `servers` holds the live handles.
    http_adapter: Option<Box<dyn HttpAdapter>>,
    http_target: Option<BindTarget>,
    container: Arc<RwLock<Container>>,
    routes: RouteMount,
    context: UloApplicationContext,
    ws_gateways: HashMap<String, Arc<GatewayWrapper>>,
    ws_adapter: Option<Box<dyn WsAdapter>>,
    /// Sockets supplied for separate-port gateways, keyed by the declared
    /// `port = N`. Drained in `bind()` as each unique port is handed to the
    /// adapter; whatever is left names a port no gateway declared.
    ws_targets: HashMap<u16, BindTarget>,
    rpc_adapter: Option<Box<dyn RpcAdapter>>,
    rpc_controllers: Vec<Arc<RpcControllerWrapper>>,
    grpc_adapter: Option<Box<dyn GrpcAdapter>>,
    /// Live lifecycle handles after `bind()`. Orchestration loops over this
    /// vector — the framework's startup/shutdown code never branches on
    /// adapter kind. Adding a new transport = pushing a new
    /// `*LifecycleHandle: ServerLifecycle` here at bind time.
    servers: Vec<Box<dyn ServerLifecycle>>,
    state: AppState,
    bound: Option<BoundState>,
    shutdown: ShutdownHandle,
}

impl UloApplication {
    pub fn new(container: Arc<RwLock<Container>>) -> Self {
        Self {
            http_adapter: None,
            http_target: None,
            context: UloApplicationContext::new(container.clone()),
            container: container.clone(),
            routes: RouteMount::new(container),
            ws_gateways: HashMap::new(),
            ws_adapter: None,
            ws_targets: HashMap::new(),
            rpc_adapter: None,
            rpc_controllers: Vec::new(),
            grpc_adapter: None,
            servers: Vec::new(),
            state: AppState::Configuring,
            bound: None,
            shutdown: ShutdownHandle::new(),
        }
    }

    fn require_state(&self, expected: AppState, op: &str) -> Result<(), StartupError> {
        if self.state != expected {
            return Err(StartupError::Setup(
                format!(
                    "{op}() cannot be called in state {:?}; expected {:?}",
                    self.state, expected
                )
                .into(),
            ));
        }
        Ok(())
    }

    /// Register an HTTP adapter listening on `target` — a `("host", port)`
    /// pair, or anything else that converts to a [`BindTarget`], such as a
    /// pre-bound `std::net::TcpListener`.
    pub fn use_http_adapter<A: HttpAdapter + 'static>(
        &mut self,
        adapter: A,
        target: impl Into<BindTarget>,
    ) -> Result<&mut Self, StartupError> {
        self.require_state(AppState::Configuring, "use_http_adapter")?;
        let mut boxed = Box::new(adapter) as Box<dyn HttpAdapter>;
        self.routes.mount(boxed.as_mut())?;
        self.http_adapter = Some(boxed);
        self.http_target = Some(target.into());
        tracing::debug!("HTTP adapter registered");
        Ok(self)
    }

    /// Gateway discovery is deferred to `bind()` to allow adapter configuration beforehand.
    pub fn use_websocket_adapter<A>(&mut self, adapter: A) -> Result<&mut Self, StartupError>
    where
        A: WsAdapter,
    {
        self.require_state(AppState::Configuring, "use_websocket_adapter")?;
        self.ws_adapter = Some(Box::new(adapter) as Box<dyn WsAdapter>);
        tracing::debug!("WebSocket adapter registered");
        Ok(self)
    }

    /// Hand a socket the caller already bound to the separate-port gateways
    /// declared with `port = declared_port`, instead of letting the adapter
    /// bind one.
    ///
    /// `declared_port` is the number written in the gateway attribute, which
    /// is what selects the gateway. The socket may be listening on a
    /// different port; [`BoundAdapters::websocket`] reports the address it
    /// listens on. Sockets acquired this way outlive the process that hands
    /// them over — a supervisor holding one across restarts (systemd socket
    /// activation, `ulo dev --listen`) leaves connections queued in the
    /// accept backlog rather than refused. Pair with the `listenfd` crate to
    /// claim an inherited descriptor.
    ///
    /// Same-port gateways ride the HTTP listener; pass their socket to
    /// [`use_http_adapter`](Self::use_http_adapter) instead.
    pub fn use_websocket_listener(
        &mut self,
        declared_port: u16,
        listener: std::net::TcpListener,
    ) -> Result<&mut Self, StartupError> {
        self.require_state(AppState::Configuring, "use_websocket_listener")?;
        self.ws_targets
            .insert(declared_port, BindTarget::Listener(listener));
        Ok(self)
    }

    pub fn use_rpc_adapter<A>(&mut self, adapter: A) -> Result<&mut Self, StartupError>
    where
        A: RpcAdapter,
    {
        self.require_state(AppState::Configuring, "use_rpc_adapter")?;
        self.rpc_adapter = Some(Box::new(adapter) as Box<dyn RpcAdapter>);
        tracing::debug!("RPC adapter registered");
        Ok(self)
    }

    /// Register a gRPC adapter. Distinct from
    /// [`use_rpc_adapter`](Self::use_rpc_adapter) because gRPC is contract-first
    /// (services are declared in `.proto` files and known at compile time)
    /// and supports streaming — neither fits the pattern-string + JSON-data
    /// model that `RpcAdapter` encodes for TCP/UDP/NATS.
    pub fn use_grpc_adapter<A>(&mut self, adapter: A) -> Result<&mut Self, StartupError>
    where
        A: GrpcAdapter,
    {
        self.require_state(AppState::Configuring, "use_grpc_adapter")?;
        self.grpc_adapter = Some(Box::new(adapter) as Box<dyn GrpcAdapter>);
        tracing::debug!("gRPC adapter registered");
        Ok(self)
    }

    /// Returns a cloneable handle for triggering and observing shutdown.
    ///
    /// The handle is valid immediately after `new()` — no need to wait for
    /// `bind()`. Calling [`ShutdownHandle::shutdown`] before `bind()` is a
    /// no-op (nothing is listening yet).
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.clone()
    }

    fn discover_gateways(&mut self) -> Result<(), StartupError> {
        let resolver = GatewayResolver::new(self.container.clone());
        self.ws_gateways = resolver.resolve()?;

        if !self.ws_gateways.is_empty() {
            tracing::debug!(
                count = self.ws_gateways.len(),
                "WebSocket gateways discovered"
            );
        }

        Ok(())
    }

    fn discover_rpc_controllers(&mut self) {
        // Wrappers are stored fully resolved at create; this only collects them for the adapter.
        self.rpc_controllers = self
            .container
            .read()
            .rpc_controllers()
            .values()
            .cloned()
            .collect();

        if !self.rpc_controllers.is_empty() {
            tracing::debug!(
                count = self.rpc_controllers.len(),
                "RPC controllers discovered"
            );
        }
    }

    /// Returns an instance of `T` from the DI container, searching across all modules.
    pub async fn get<T: 'static>(&self) -> Result<T, ResolutionError> {
        self.context.get::<T>().await
    }

    /// Returns an instance of `T` from a specific module's scope in the DI container.
    pub async fn get_from<T: 'static>(&self, module_token: &str) -> Result<T, ResolutionError> {
        self.context.get_from::<T>(module_token).await
    }

    /// Returns the value under the marker `K`, searching across all modules. See
    /// [`UloApplicationContext::get_key`](crate::application_context::UloApplicationContext::get_key).
    pub async fn get_key<K>(&self) -> Result<K::Value, ResolutionError>
    where
        K: crate::di::Key,
        K::Value: Sized,
    {
        self.context.get_key::<K>().await
    }

    /// Returns the value under the marker `K` from a specific module's scope.
    pub async fn get_from_key<K>(&self, module_token: &str) -> Result<K::Value, ResolutionError>
    where
        K: crate::di::Key,
        K::Value: Sized,
    {
        self.context.get_from_key::<K>(module_token).await
    }

    /// Returns an instance from the DI container by token rather than type; use when providers
    /// are registered with a custom token.
    pub async fn get_by_token<T: 'static>(
        &self,
        token: impl IntoToken<T>,
    ) -> Result<T, ResolutionError> {
        self.context.get_by_token::<T>(token).await
    }

    /// Returns an instance by token from a specific module's scope in the DI container.
    pub async fn get_from_by_token<T: 'static>(
        &self,
        module_token: &str,
        token: impl IntoToken<T>,
    ) -> Result<T, ResolutionError> {
        self.context
            .get_from_by_token::<T>(module_token, token)
            .await
    }

    /// The module handle for `M`, found by its identity. See
    /// [`UloApplicationContext::get_module`](crate::application_context::UloApplicationContext::get_module).
    pub async fn get_module<M: 'static>(
        &self,
    ) -> Result<crate::di::internal::ModuleRef, ResolutionError> {
        self.context.get_module::<M>().await
    }

    /// The module handle for the module whose identity key or base is `id`. See
    /// [`UloApplicationContext::get_module_by_id`](crate::application_context::UloApplicationContext::get_module_by_id).
    pub async fn get_module_by_id(
        &self,
        id: &str,
    ) -> Result<crate::di::internal::ModuleRef, ResolutionError> {
        self.context.get_module_by_id(id).await
    }

    /// Resolves a provider `T` in an execution.
    ///
    /// Everything resolved in one execution shares its cache, so an execution-scoped
    /// provider is built once for all of them. Use
    /// [`Execution::standalone`](crate::di::Execution::standalone) where the
    /// work arrived over no transport.
    pub async fn resolve<T: 'static>(
        &self,
        execution: &crate::di::Execution,
    ) -> Result<T, ResolutionError> {
        self.context.resolve::<T>(execution).await
    }

    /// Resolves the value under the marker `K` in an execution.
    pub async fn resolve_key<K>(
        &self,
        execution: &crate::di::Execution,
    ) -> Result<K::Value, ResolutionError>
    where
        K: crate::di::Key,
        K::Value: Sized,
    {
        self.context.resolve_key::<K>(execution).await
    }

    /// Resolves a provider by token in an execution.
    pub async fn resolve_by_token<T: 'static>(
        &self,
        token: impl IntoToken<T>,
        execution: &crate::di::Execution,
    ) -> Result<T, ResolutionError> {
        self.context.resolve_by_token::<T>(token, execution).await
    }

    /// Bind all registered adapters and run bootstrap hooks.
    ///
    /// Sockets are live and listening when this returns. The actual serve loops
    /// are started by [`run`](UloApplication::run). Call `bind()` to get the
    /// bound addresses (e.g. when port 0 was passed and the OS assigned one),
    /// then call `run()` to block until shutdown.
    ///
    /// Every transport the application declares must come up. An adapter that
    /// fails to take its handlers or to acquire its socket returns
    /// [`StartupError::Adapter`]; a declaration with nothing registered to serve it
    /// returns [`StartupError::Setup`]. Starting anyway would leave a process that
    /// answers less than it advertises while every liveness signal reads
    /// healthy, and `BoundAdapters` cannot express the difference — its `rpc`
    /// field is `None` both for a dead adapter and for a subject-based
    /// transport that has no address to report.
    ///
    /// Every adapter is asked to take what the application declares before any of
    /// them is asked for a socket, so a declaration that cannot work fails with
    /// nothing acquired. An application that is wrong therefore reports that
    /// before an environment that is busy.
    ///
    /// Sockets acquired before a failure are closed on the way out, and the
    /// application cannot be bound again.
    pub async fn bind(&mut self) -> Result<BoundAdapters, StartupError> {
        self.require_state(AppState::Configuring, "bind")?;

        match self.bind_adapters().await {
            Ok(bound) => {
                self.state = AppState::Bound;
                Ok(bound)
            }
            Err(e) => {
                self.state = AppState::Failed;
                // Closing gives each adapter that did come up its chance to
                // drain; dropping the handles afterwards releases the sockets,
                // which would otherwise stay open for as long as the caller
                // holds the application.
                self.close_adapters().await;
                self.servers.clear();
                self.bound = None;
                Err(e)
            }
        }
    }

    /// Registration, then acquisition — never interleaved.
    ///
    /// Every adapter is asked to take what the application declares before any of them is asked
    /// for a socket, so a declaration that cannot work fails with nothing acquired and the
    /// teardown in [`bind`](UloApplication::bind) only ever handles sockets. It also fixes the
    /// order failures are reported in: an application that is wrong reports that before an
    /// environment that is busy.
    async fn bind_adapters(&mut self) -> Result<BoundAdapters, StartupError> {
        {
            let mut scanner =
                crate::di::internal::scanner::DependencyScanner::new(self.container.clone());
            scanner.call_bootstrap_hooks().await?;
        }

        self.discover_gateways()?;
        self.discover_rpc_controllers();

        // ── Registration ────────────────────────────────────────────────────
        // Nothing below this point until the acquisition section touches a socket.

        // For a pre-bound Listener target the hint is the actual bound port,
        // so a gateway declaring that port number still rides the HTTP
        // listener. The hostname feeds separate-port WS binds; a Listener
        // target carries no hostname, so those fall back to all-interfaces.
        let http_port = self.http_target.as_ref().and_then(|t| t.port_hint());
        let hostname = match self.http_target.as_ref() {
            Some(BindTarget::Addr { hostname, .. }) => hostname.clone(),
            _ => "0.0.0.0".to_string(),
        };

        // One shared WsClientMap + ConnectionManager when BroadcastService is in DI;
        // otherwise a fresh WsClientMap per gateway (no CM needed).
        let broadcast_service = self
            .context
            .get::<BroadcastService>()
            .await
            .ok()
            .map(Arc::new);

        // Same-port vs separate-port is a property of how the gateway was declared,
        // not of the port number. A gateway with no `port` shares the HTTP listener;
        // any `port = N` (including 0) wants its own. Port 0 means the OS assigns
        // distinct ports, so a gateway requesting 0 is always separate-port even
        // when HTTP also requested 0 — comparing literal numbers conflates intent
        // with coincidence and breaks at 0.
        let (same_port, separate_port): (Vec<_>, Vec<_>) = self
            .ws_gateways
            .iter()
            .map(|(p, gw)| (p.clone(), gw.clone()))
            .partition(|(_, gw)| {
                let p = gw.port();
                p.is_none() || http_port.map_or(false, |hp| hp != 0 && p == Some(hp))
            });

        // Wire same-port gateways into the HTTP adapter as upgrade routes.
        if !same_port.is_empty() {
            let Some(http) = self.http_adapter.as_mut() else {
                let paths: Vec<&str> = same_port.iter().map(|(p, _)| p.as_str()).collect();
                return Err(StartupError::Setup(
                    format!(
                        "WebSocket gateways at {} share the HTTP listener, but no HTTP adapter is \
                     registered; call use_http_adapter() to add one",
                        paths.join(", ")
                    )
                    .into(),
                )
                .into());
            };

            for (path, gateway) in &same_port {
                let client_map = broadcast_service
                    .as_ref()
                    .map(|bs| bs.ws_client_map())
                    .unwrap_or_else(|| Arc::new(WsClientMap::new()));
                let callbacks = Arc::new(make_ws_callbacks(
                    gateway.clone(),
                    client_map,
                    broadcast_service.clone(),
                ));
                // Upgrade requests arrive with trailing slashes already
                // trimmed (ServeContext), so register the trimmed form.
                let trimmed = crate::http::trim_trailing_slashes(path);
                http.register_ws_route(trimmed, callbacks)
                    .map_err(|source| StartupError::Adapter {
                        transport: "websocket",
                        source: source.into(),
                    })?;
                tracing::debug!(path, "WebSocket gateway registered");
                gateway.call_after_init().await;
            }
        }

        // Wire separate-port gateways into the standalone WS adapter, and work out
        // which socket each declared port gets. The adapter is consumed further
        // down; here it only takes its gateways.
        let ws_targets: Option<Vec<(u16, BindTarget)>> =
            if separate_port.is_empty() {
                None
            } else {
                let Some(ws) = self.ws_adapter.as_mut() else {
                    let declared: Vec<String> = separate_port
                        .iter()
                        .map(|(path, gw)| format!("{path} (port {})", gw.port().unwrap_or(0)))
                        .collect();
                    return Err(StartupError::Setup(format!(
                    "WebSocket gateways {} declare their own port, but no WebSocket adapter is \
                     registered; call use_websocket_adapter() to add one",
                    declared.join(", ")
                ).into())
                .into());
                };

                for (path, gateway) in &separate_port {
                    if let Some(ws_port) = gateway.port() {
                        let client_map = broadcast_service
                            .as_ref()
                            .map(|bs| bs.ws_client_map())
                            .unwrap_or_else(|| Arc::new(WsClientMap::new()));
                        let callbacks = Arc::new(make_ws_callbacks(
                            gateway.clone(),
                            client_map,
                            broadcast_service.clone(),
                        ));
                        ws.register_gateway(ws_port, path, callbacks)
                            .map_err(|source| StartupError::Adapter {
                                transport: "websocket",
                                source: source.into(),
                            })?;
                        tracing::debug!(port = ws_port, path, "WebSocket gateway registered");
                        gateway.call_after_init().await;
                    }
                }

                // Collect every unique port that has at least one gateway. A gateway
                // keeps its declared port as its key even when the caller supplied a
                // socket listening elsewhere — the key selects the gateway, the target
                // says where to listen.
                let mut seen: HashSet<u16> = HashSet::new();
                let mut targets: Vec<(u16, BindTarget)> = vec![];
                for (_, gw) in &separate_port {
                    if let Some(ws_port) = gw.port() {
                        if seen.insert(ws_port) {
                            let target =
                                self.ws_targets
                                    .remove(&ws_port)
                                    .unwrap_or(BindTarget::Addr {
                                        hostname: hostname.clone(),
                                        port: ws_port,
                                    });
                            targets.push((ws_port, target));
                        }
                    }
                }
                Some(targets)
            };

        // A socket left here matches no gateway, so nothing will ever accept
        // on it.
        if !self.ws_targets.is_empty() {
            let orphans: Vec<String> = self
                .ws_targets
                .drain()
                .map(|(declared_port, target)| format!("{target} for port {declared_port}"))
                .collect();
            return Err(StartupError::Setup(
                format!(
                    "WebSocket listeners supplied for ports no gateway declares: {}",
                    orphans.join(", ")
                )
                .into(),
            )
            .into());
        }

        // Hand the RPC adapter its patterns. It is consumed further down.
        let rpc_adapter =
            if self.rpc_controllers.is_empty() {
                None
            } else {
                if self.rpc_adapter.is_none() {
                    return Err(StartupError::Setup(format!(
                    "{} RPC controller(s) declare patterns, but no RPC adapter is registered; \
                     call use_rpc_adapter() to add one",
                    self.rpc_controllers.len()
                ).into())
                .into());
                }

                let all_patterns: Vec<String> = self
                    .rpc_controllers
                    .iter()
                    .flat_map(|w| w.patterns())
                    .collect();

                for pattern in &all_patterns {
                    tracing::debug!(pattern = %pattern, "RPC pattern registered");
                }

                let rpc_global_handlers = self.container.read().global_rpc.error_handlers.clone();
                let callbacks = Arc::new(make_rpc_callbacks(
                    self.rpc_controllers.clone(),
                    rpc_global_handlers,
                ));
                let mut adapter = self.rpc_adapter.take().unwrap();
                adapter
                    .register_handlers(&all_patterns, callbacks)
                    .map_err(|source| StartupError::Adapter {
                        transport: "rpc",
                        source: source.into(),
                    })?;
                Some(adapter)
            };

        // Hand the gRPC adapter its services. Services declared with
        // `#[controller]` + `#[grpc_methods]` are picked up from the DI
        // container; users may also wire services directly on the adapter via
        // its own `add_service` builder before `use_grpc_adapter`.
        let grpc_adapter = if let Some(mut adapter) = self.grpc_adapter.take() {
            // Bundles are stored fully resolved at create; this only hands them to the adapter.
            let grpc_services: Vec<_> = self
                .container
                .read()
                .grpc_services()
                .values()
                .cloned()
                .collect();
            adapter
                .register_services(grpc_services)
                .map_err(|source| StartupError::Adapter {
                    transport: "grpc",
                    source: source.into(),
                })?;
            Some(adapter)
        } else {
            None
        };

        // ── Acquisition ─────────────────────────────────────────────────────
        // Every adapter has taken what it was given; from here sockets are opened.

        let mut ws_addrs: Vec<SocketAddr> = vec![];
        if let Some(targets) = ws_targets {
            // The adapter is moved into `SharedWsAdapter` so each per-port handle
            // can call close() idempotently.
            let adapter = self.ws_adapter.take().unwrap();
            let handles = adapter
                .into_lifecycle_handles(targets)
                .await
                .map_err(|source| StartupError::Adapter {
                    transport: "websocket",
                    source: source.into(),
                })?;
            for handle in handles {
                let addr = handle.local_addr();
                tracing::info!(addr = %addr, "WebSocket listening");
                ws_addrs.push(addr);
                self.servers.push(Box::new(handle));
            }
        }

        let mut rpc_addr: Option<SocketAddr> = None;
        if let Some(adapter) = rpc_adapter {
            let handle =
                adapter
                    .into_lifecycle()
                    .await
                    .map_err(|source| StartupError::Adapter {
                        transport: "rpc",
                        source: source.into(),
                    })?;
            rpc_addr = handle.local_addr();
            self.servers.push(Box::new(handle));
        }

        let mut grpc_addr: Option<SocketAddr> = None;
        if let Some(adapter) = grpc_adapter {
            let handle =
                adapter
                    .into_lifecycle()
                    .await
                    .map_err(|source| StartupError::Adapter {
                        transport: "grpc",
                        source: source.into(),
                    })?;
            grpc_addr = handle.local_addr();
            if let Some(addr) = grpc_addr {
                tracing::info!(addr = %addr, "gRPC listening");
            }
            self.servers.push(Box::new(handle));
        }

        let http_addr =
            if let Some(http_adapter) = self.http_adapter.take() {
                let target = self.http_target.take().unwrap();
                let has_same_port_ws = !same_port.is_empty();
                let server_type = if has_same_port_ws {
                    "HTTP + WebSocket"
                } else {
                    "HTTP"
                };

                let ctx = ServeContext::new(self.routes.take_global_chain());
                let handle = http_adapter
                    .into_lifecycle(target, ctx)
                    .await
                    .map_err(|source| StartupError::Adapter {
                        transport: "http",
                        source: source.into(),
                    })?;
                let addr = handle
                    .local_addr()
                    .expect("HTTP handle always has a bound address");
                tracing::info!(addr = %addr, server_type, "HTTP listening");
                self.servers.push(Box::new(handle));
                Some(addr)
            } else if self.servers.is_empty() {
                return Err(StartupError::Setup(format!(
                "No adapters configured; register at least one adapter before calling bind()"
            ).into())
            .into());
            } else {
                None
            };

        // Drain serve futures out of every handle now so `run()` can join them
        // all. After this point, handles still in `self.servers` are used only
        // for `shutdown()`.
        let serve_futures: Vec<_> = self
            .servers
            .iter_mut()
            .filter_map(|s| s.take_serve())
            .collect();

        self.bound = Some(BoundState { serve_futures });

        Ok(BoundAdapters {
            http: http_addr,
            websocket: ws_addrs,
            rpc: rpc_addr,
            grpc: grpc_addr,
        })
    }

    /// Bind all adapters and drive the serve loops until shutdown.
    ///
    /// Convenience wrapper over [`bind`](UloApplication::bind) +
    /// [`run`](UloApplication::run). Use this when you don't need the bound
    /// address; use `bind()` + `run()` explicitly when you do (dynamic ports,
    /// tests, readiness probes).
    pub async fn start(mut self) -> Result<(), StartupError> {
        self.bind().await?;
        self.run().await;
        Ok(())
    }

    /// Drive the serve loops until [`ShutdownHandle::shutdown`] is called.
    ///
    /// Consumes `self`. On a shutdown signal, adapters are closed first so
    /// in-flight requests can drain before lifecycle hooks run.
    ///
    /// A serve loop that returns on its own signals shutdown for the whole
    /// application. An adapter stops accepting only when it is shutting down or
    /// when its transport has failed, and a process that keeps serving the
    /// remaining transports after one dies answers less than it advertises.
    ///
    /// # Panics
    ///
    /// Panics if called before [`bind`](UloApplication::bind). Use
    /// [`start`](UloApplication::start) to bind and run in one step.
    pub async fn run(mut self) {
        let bound = self
            .bound
            .take()
            .expect("run() called before bind() — call bind() first or use start()");

        let shutdown = self.shutdown.clone();

        // Signalling from inside each serve future is what makes one dead
        // transport close the application: during a graceful shutdown the flag
        // is already set and this is a no-op, so only an unexpected return
        // actually triggers anything.
        let serve_futures: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = bound
            .serve_futures
            .into_iter()
            .map(|serve| {
                let shutdown = shutdown.clone();
                Box::pin(async move {
                    serve.await;
                    shutdown.shutdown();
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            })
            .collect();

        let serve_all = Box::pin(futures::future::join_all(serve_futures));
        let shutdown_wait = Box::pin(async move { shutdown.wait_for_shutdown().await });

        match futures::future::select(serve_all, shutdown_wait).await {
            futures::future::Either::Left(_) => {
                // Serve loops exited naturally — run lifecycle hooks then close adapters.
                self.close().await;
            }
            futures::future::Either::Right((_, serve_all)) => {
                // Shutdown signalled — close adapters so accept loops exit, drain,
                // then run lifecycle hooks.
                tracing::info!("Application shutting down");
                self.close_adapters().await;
                serve_all.await;
                self.close_hooks().await;
                tracing::info!("Application shutdown complete");
            }
        }

        self.shutdown.mark_completed();
    }

    /// Immediately run lifecycle hooks and close all adapters.
    ///
    /// Prefer triggering shutdown via [`ShutdownHandle::shutdown`] so that
    /// in-flight requests are given a chance to drain. Call `close()` directly
    /// only when an immediate stop is required.
    pub async fn close(&mut self) {
        tracing::info!("Application shutting down");
        self.close_hooks().await;
        self.close_adapters().await;
        tracing::info!("Application shutdown complete");
    }

    async fn close_hooks(&mut self) {
        self.context.call_module_destroy_hooks().await;
        self.context.call_before_shutdown_hooks(None).await;
        self.context.call_shutdown_hooks(None).await;
    }

    async fn close_adapters(&mut self) {
        if let Ok(bs) = self.context.get::<BroadcastService>().await {
            bs.close_all().await;
        }

        // Reverse order — last registered is first closed. Each handle is
        // an opaque `Box<dyn ServerLifecycle>`; the framework's shutdown
        // code doesn't know which transport it's draining.
        for handle in self.servers.iter_mut().rev() {
            let name = handle.name();
            if let Err(e) = handle.shutdown().await {
                tracing::warn!(server = name, error = %e, "adapter close error");
            }
        }
    }
}

/// Build the connection callbacks for one gateway.
///
/// `client_map` is either the shared map from `BroadcastService` (when BS is in DI) or
/// a fresh per-gateway map (when the user hasn't imported `BroadcastModule`).
/// `broadcast_service` is `Some` only when BS is in DI; the CM is wired through it.
fn make_ws_callbacks(
    gateway: Arc<GatewayWrapper>,
    client_map: Arc<WsClientMap>,
    broadcast_service: Option<Arc<BroadcastService>>,
) -> WsConnectionCallbacks {
    let g_connect = gateway.clone();
    let g_message = gateway.clone();
    let g_disconnect = gateway.clone();
    let h_message = client_map.clone();
    let h_disconnect = client_map.clone();
    let bs_connect = broadcast_service.clone();
    let bs_disconnect = broadcast_service;

    WsConnectionCallbacks::new(
        move |parts, sink| {
            let gateway = g_connect.clone();
            let bs = bs_connect.clone();
            let map = client_map.clone();
            Box::pin(async move {
                let client = create_client_from_parts(&parts);
                let client_id = client.id.clone();
                // The sink is registered between the two phases, inside the one connect execution
                // the context carries — so what a guard wrote is still there for the hook.
                let context = gateway.begin_connect(client).await?;
                if let Some(bs) = &bs {
                    bs.connect(client_id.clone(), sink, gateway.namespace());
                } else {
                    map.register(client_id.clone(), sink);
                }
                gateway.complete_connect(&context).await?;
                Ok(client_id)
            })
        },
        move |client_id, msg| {
            let gateway = g_message.clone();
            let handle = h_message.clone();
            Box::pin(async move {
                match gateway.handle_message(client_id.clone(), msg).await {
                    Ok(Items::Empty) => MessageCallbackResult::Continue,
                    Ok(Items::One(response)) => {
                        handle.send_to(&client_id, response).await;
                        MessageCallbackResult::Continue
                    }
                    // The adapter SPI carries what the wire carries. A WebSocket item's error
                    // type is `Infallible`, so unwrapping one here is total: there is no value of
                    // that type for the `Err` arm to be given.
                    Ok(Items::Many(stream)) => {
                        use futures::StreamExt as _;
                        MessageCallbackResult::Stream(
                            stream
                                .map(|item| match item {
                                    Ok(message) => message,
                                    Err(never) => match never {},
                                })
                                .boxed(),
                        )
                    }
                    // The gateway answers every failure it can answer, frame included, so what
                    // reaches here is the one case with nothing left to answer on: the client
                    // is gone. Stop reading from it.
                    Err(_) => MessageCallbackResult::Stop,
                }
            })
        },
        move |client_id| {
            let gateway = g_disconnect.clone();
            let handle = h_disconnect.clone();
            let bs = bs_disconnect.clone();
            Box::pin(async move {
                if let Some(bs) = &bs {
                    bs.disconnect(&client_id);
                } else {
                    handle.unregister(&client_id);
                }
                gateway
                    .handle_disconnect(client_id, DisconnectReason::ClientDisconnect)
                    .await;
            })
        },
    )
}

/// Build the message callbacks for all RPC controllers.
///
/// Constructs a pattern → wrapper index at call time so the hot path
/// (per-message dispatch) is a single HashMap lookup.
///
/// The global error handlers are threaded in for the miss: a pattern no
/// controller claims has no wrapper to run, so a `#[catch(Unrouted)]` handler
/// would otherwise never see the one call an operator most wants to hear about.
fn make_rpc_callbacks(
    wrappers: Vec<Arc<RpcControllerWrapper>>,
    global_error_handlers: Vec<
        crate::dispatch::transport::ErrorHandlerArc<crate::dispatch::transport::Rpc>,
    >,
) -> RpcMessageCallbacks {
    let mut pattern_map: HashMap<String, Arc<RpcControllerWrapper>> = HashMap::new();
    for wrapper in &wrappers {
        for pattern in wrapper.patterns() {
            pattern_map.insert(pattern, wrapper.clone());
        }
    }
    let pattern_map = Arc::new(pattern_map);

    use crate::rpc::RpcContext;

    let global_error_handlers = Arc::new(global_error_handlers);

    RpcMessageCallbacks::new(move |data: RpcData, info: RpcCallInfo| {
        let pattern_map = pattern_map.clone();
        let global_error_handlers = global_error_handlers.clone();
        Box::pin(async move {
            if let Some(wrapper) = pattern_map.get(&info.pattern) {
                return wrapper.handle_message(data, info).await;
            }

            // A context is built for the miss so a handler that claims the
            // event can read the headers the caller sent.
            let event = crate::errors::Unrouted::new(info.pattern.clone());
            let ctx = RpcContext::with_extensions(
                info.pattern.clone(),
                data,
                info.headers,
                None,
                info.extensions,
            );
            if let Some(claimed) =
                crate::enhancer::pipeline::claim::<crate::dispatch::transport::Rpc>(
                    &global_error_handlers,
                    &event,
                    &ctx,
                )
                .await
            {
                return claimed;
            }
            Err(RpcError::PatternNotFound(info.pattern))
        })
    })
}

#[cfg(test)]
mod send_invariant {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_send_future<F: Future + Send>(_: F) {}

    #[test]
    fn an_application_and_its_context_are_send() {
        assert_send::<UloApplication>();
        assert_send::<UloApplicationContext>();
    }

    #[allow(dead_code)]
    fn serving_futures_are_send(app: UloApplication, mut ctx: UloApplicationContext) {
        assert_send_future(app.start());
        assert_send_future(async move { ctx.close().await });
    }
}

#[cfg(test)]
mod shutdown_handle_debug {
    use super::*;

    #[test]
    fn debug_reports_the_two_observable_flags() {
        let handle = ShutdownHandle::new();
        assert_eq!(
            format!("{handle:?}"),
            "ShutdownHandle { is_shutdown: false, completed: false, .. }"
        );

        handle.shutdown();
        assert_eq!(
            format!("{handle:?}"),
            "ShutdownHandle { is_shutdown: true, completed: false, .. }"
        );
    }

    /// The shape that wanted the bound: `oneshot::Sender::send` answers `Result<(), T>`, so
    /// handing a handle to a waiting task and unwrapping the send needs `T: Debug`.
    #[test]
    fn a_handle_rides_the_error_side_of_a_result() {
        let sent: Result<(), ShutdownHandle> = Ok(());
        sent.unwrap();
    }
}
