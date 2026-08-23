use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::error::BindError;
use anyhow::Result;
use event_listener::Event;

use crate::{
    adapter::{
        AdapterContext, BindTarget, GrpcAdapter, HttpAdapter, MessageCallbackResult, RpcAdapter,
        RpcMessageCallbacks, WebSocketAdapter, WsConnectionCallbacks,
        server_lifecycle::ServerLifecycle,
    },
    application_context::ToniApplicationContext,
    injector::{
        GatewayResolver, GrpcServiceResolver, IntoToken, RpcControllerResolver, ToniContainer,
    },
    router::RoutesResolver,
    rpc::{RpcCallInfo, RpcControllerWrapper, RpcData, RpcError},
    websocket::{
        BroadcastService, DisconnectReason, GatewayWrapper, WsClientMap, WsError, WsHandlerOutput,
        WsMessage, helpers::create_client_from_parts,
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
/// Created by [`ToniApplication::shutdown_handle`] and usable from any task.
/// Calling [`shutdown`](ShutdownHandle::shutdown) is idempotent — multiple
/// callers are safe.
#[derive(Clone)]
pub struct ShutdownHandle {
    inner: Arc<ShutdownInner>,
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

    /// Resolves once [`run`](ToniApplication::run) has returned — i.e. all
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

/// The addresses of all bound adapters, returned by [`ToniApplication::bind`].
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
    /// failed to start, which fails [`bind`](ToniApplication::bind) instead.
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

pub struct ToniApplication {
    // Adapters live here in `Configuring` state, before `bind()` consumes
    // them into lifecycle handles. After `bind()` succeeds these are all
    // `None` and `servers` holds the live handles.
    http_adapter: Option<Box<dyn HttpAdapter>>,
    http_target: Option<BindTarget>,
    routes_resolver: RoutesResolver,
    context: ToniApplicationContext,
    ws_gateways: HashMap<String, Arc<GatewayWrapper>>,
    ws_adapter: Option<Box<dyn WebSocketAdapter>>,
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

impl ToniApplication {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self {
            http_adapter: None,
            http_target: None,
            context: ToniApplicationContext::new(container.clone()),
            routes_resolver: RoutesResolver::new(container),
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

    fn require_state(&self, expected: AppState, op: &str) -> Result<()> {
        if self.state != expected {
            anyhow::bail!(
                "{op}() cannot be called in state {:?}; expected {:?}",
                self.state,
                expected
            );
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
    ) -> Result<&mut Self> {
        self.require_state(AppState::Configuring, "use_http_adapter")?;
        let mut boxed = Box::new(adapter) as Box<dyn HttpAdapter>;
        self.routes_resolver.resolve(boxed.as_mut())?;
        self.http_adapter = Some(boxed);
        self.http_target = Some(target.into());
        tracing::debug!("HTTP adapter registered");
        Ok(self)
    }

    /// Gateway discovery is deferred to `bind()` to allow adapter configuration beforehand.
    pub fn use_websocket_adapter<A>(&mut self, adapter: A) -> Result<&mut Self>
    where
        A: WebSocketAdapter,
    {
        self.require_state(AppState::Configuring, "use_websocket_adapter")?;
        self.ws_adapter = Some(Box::new(adapter) as Box<dyn WebSocketAdapter>);
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
    /// activation, `toni dev --listen`) leaves connections queued in the
    /// accept backlog rather than refused. Pair with the `listenfd` crate to
    /// claim an inherited descriptor.
    ///
    /// Same-port gateways ride the HTTP listener; pass their socket to
    /// [`use_http_adapter`](Self::use_http_adapter) instead.
    pub fn use_websocket_listener(
        &mut self,
        declared_port: u16,
        listener: std::net::TcpListener,
    ) -> Result<&mut Self> {
        self.require_state(AppState::Configuring, "use_websocket_listener")?;
        self.ws_targets
            .insert(declared_port, BindTarget::Listener(listener));
        Ok(self)
    }

    pub fn use_rpc_adapter<A>(&mut self, adapter: A) -> Result<&mut Self>
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
    pub fn use_grpc_adapter<A>(&mut self, adapter: A) -> Result<&mut Self>
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

    fn discover_gateways(&mut self) -> Result<()> {
        let resolver = GatewayResolver::new(self.routes_resolver.container.clone());
        self.ws_gateways = resolver.resolve()?;

        if !self.ws_gateways.is_empty() {
            tracing::debug!(
                count = self.ws_gateways.len(),
                "WebSocket gateways discovered"
            );
        }

        Ok(())
    }

    fn discover_rpc_controllers(&mut self) -> Result<()> {
        let resolver = RpcControllerResolver::new(self.routes_resolver.container.clone());
        self.rpc_controllers = resolver.resolve()?;

        if !self.rpc_controllers.is_empty() {
            tracing::debug!(
                count = self.rpc_controllers.len(),
                "RPC controllers discovered"
            );
        }

        Ok(())
    }

    /// Returns an instance of `T` from the DI container, searching across all modules.
    pub async fn get<T: 'static>(&self) -> Result<T> {
        self.context.get::<T>().await
    }

    /// Returns an instance of `T` from a specific module's scope in the DI container.
    pub async fn get_from<T: 'static>(&self, module_token: &str) -> Result<T> {
        self.context.get_from::<T>(module_token).await
    }

    /// Returns an instance from the DI container by token rather than type; use when providers
    /// are registered with a custom token.
    pub async fn get_by_token<T: 'static>(&self, token: impl IntoToken) -> Result<T> {
        self.context.get_by_token::<T>(token).await
    }

    /// Returns an instance by token from a specific module's scope in the DI container.
    pub async fn get_from_by_token<T: 'static>(
        &self,
        module_token: &str,
        token: impl IntoToken,
    ) -> Result<T> {
        self.context
            .get_from_by_token::<T>(module_token, token)
            .await
    }

    /// Resolves a provider `T` in an execution.
    ///
    /// Everything resolved in one execution shares its cache, so a request-scoped
    /// provider is built once for all of them. Use
    /// [`ProviderContext::standalone`](crate::ProviderContext::standalone) where the
    /// work arrived over no transport.
    pub async fn resolve<T: 'static>(
        &self,
        execution: &crate::traits_helpers::ProviderContext,
    ) -> Result<T> {
        self.context.resolve::<T>(execution).await
    }

    /// Resolves a provider by token in an execution.
    pub async fn resolve_by_token<T: 'static>(
        &self,
        token: impl IntoToken,
        execution: &crate::traits_helpers::ProviderContext,
    ) -> Result<T> {
        self.context.resolve_by_token::<T>(token, execution).await
    }

    /// Bind all registered adapters and run bootstrap hooks.
    ///
    /// Sockets are live and listening when this returns. The actual serve loops
    /// are started by [`run`](ToniApplication::run). Call `bind()` to get the
    /// bound addresses (e.g. when port 0 was passed and the OS assigned one),
    /// then call `run()` to block until shutdown.
    ///
    /// Every transport the application declares must come up. An adapter that
    /// fails to take its handlers or to acquire its socket returns
    /// [`BindError::Adapter`]; a declaration with nothing registered to serve it
    /// returns [`BindError::Setup`]. Starting anyway would leave a process that
    /// answers less than it advertises while every liveness signal reads
    /// healthy, and `BoundAdapters` cannot express the difference — its `rpc`
    /// field is `None` both for a dead adapter and for a subject-based
    /// transport that has no address to report.
    ///
    /// Sockets acquired before the failure are closed on the way out, and the
    /// application cannot be bound again.
    pub async fn bind(&mut self) -> Result<BoundAdapters, BindError> {
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

    async fn bind_adapters(&mut self) -> Result<BoundAdapters, BindError> {
        {
            let mut scanner = crate::scanner::ToniDependenciesScanner::new(
                self.routes_resolver.container.clone(),
            );
            scanner.call_bootstrap_hooks().await?;
        }

        self.discover_gateways()?;
        self.discover_rpc_controllers()?;

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
        let broadcast_service = self.get::<BroadcastService>().await.ok().map(Arc::new);

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
                let p = gw.get_port();
                p.is_none() || http_port.map_or(false, |hp| hp != 0 && p == Some(hp))
            });

        // Wire same-port gateways into the HTTP adapter as upgrade routes.
        if !same_port.is_empty() {
            let Some(http) = self.http_adapter.as_mut() else {
                let paths: Vec<&str> = same_port.iter().map(|(p, _)| p.as_str()).collect();
                return Err(anyhow::anyhow!(
                    "WebSocket gateways at {} share the HTTP listener, but no HTTP adapter is \
                     registered; call use_http_adapter() to add one",
                    paths.join(", ")
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
                // trimmed (AdapterContext), so register the trimmed form.
                let trimmed = crate::http_helpers::trim_trailing_slashes(path);
                http.register_ws_route(trimmed, callbacks)
                    .map_err(|source| BindError::Adapter {
                        transport: "websocket",
                        source: source.into(),
                    })?;
                tracing::debug!(path, "WebSocket gateway registered");
                gateway.call_after_init().await;
            }
        }

        let mut ws_addrs: Vec<SocketAddr> = vec![];

        // Wire separate-port gateways into the standalone WS adapter.
        // The adapter is moved into `SharedWsAdapter` so each per-port handle
        // can call close() idempotently.
        if !separate_port.is_empty() {
            if self.ws_adapter.is_none() {
                let declared: Vec<String> = separate_port
                    .iter()
                    .map(|(path, gw)| format!("{path} (port {})", gw.get_port().unwrap_or(0)))
                    .collect();
                return Err(anyhow::anyhow!(
                    "WebSocket gateways {} declare their own port, but no WebSocket adapter is \
                     registered; call use_websocket_adapter() to add one",
                    declared.join(", ")
                )
                .into());
            }

            // Register all paths on the adapter while we still own it.
            {
                let ws = self.ws_adapter.as_mut().unwrap();
                for (path, gateway) in &separate_port {
                    if let Some(ws_port) = gateway.get_port() {
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
                            .map_err(|source| BindError::Adapter {
                                transport: "websocket",
                                source: source.into(),
                            })?;
                        tracing::debug!(port = ws_port, path, "WebSocket gateway registered");
                        gateway.call_after_init().await;
                    }
                }

                // Collect every unique port that has at least one
                // gateway, then consume the adapter to produce one
                // lifecycle handle per port. A gateway keeps its declared
                // port as its key even when the caller supplied a socket
                // listening elsewhere — the key selects the gateway, the
                // target says where to listen.
                let mut seen: HashSet<u16> = HashSet::new();
                let mut targets: Vec<(u16, BindTarget)> = vec![];
                for (_, gw) in &separate_port {
                    if let Some(ws_port) = gw.get_port() {
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
                let adapter = self.ws_adapter.take().unwrap();
                let handles = adapter
                    .into_lifecycle_handles(targets)
                    .await
                    .map_err(|source| BindError::Adapter {
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
        }

        // A socket left here matches no gateway, so nothing will ever accept
        // on it.
        if !self.ws_targets.is_empty() {
            let orphans: Vec<String> = self
                .ws_targets
                .drain()
                .map(|(declared_port, target)| format!("{target} for port {declared_port}"))
                .collect();
            return Err(anyhow::anyhow!(
                "WebSocket listeners supplied for ports no gateway declares: {}",
                orphans.join(", ")
            )
            .into());
        }

        // Wire RPC controllers into the RPC adapter.
        let mut rpc_addr: Option<SocketAddr> = None;
        if !self.rpc_controllers.is_empty() {
            if self.rpc_adapter.is_none() {
                return Err(anyhow::anyhow!(
                    "{} RPC controller(s) declare patterns, but no RPC adapter is registered; \
                     call use_rpc_adapter() to add one",
                    self.rpc_controllers.len()
                )
                .into());
            }

            let all_patterns: Vec<String> = self
                .rpc_controllers
                .iter()
                .flat_map(|w| w.get_patterns())
                .collect();

            for pattern in &all_patterns {
                tracing::debug!(pattern = %pattern, "RPC pattern registered");
            }

            let callbacks = Arc::new(make_rpc_callbacks(self.rpc_controllers.clone()));
            let mut adapter = self.rpc_adapter.take().unwrap();
            adapter
                .register_handlers(&all_patterns, callbacks)
                .map_err(|source| BindError::Adapter {
                    transport: "rpc",
                    source: source.into(),
                })?;
            let handle = adapter
                .into_lifecycle()
                .await
                .map_err(|source| BindError::Adapter {
                    transport: "rpc",
                    source: source.into(),
                })?;
            rpc_addr = handle.local_addr();
            self.servers.push(Box::new(handle));
        }

        // Wire the gRPC adapter. Services declared with `#[grpc_service]` +
        // `#[grpc_methods]` are picked up from the DI container; users may
        // also wire services directly on the adapter via its own
        // `add_service` builder before `use_grpc_adapter`.
        let mut grpc_addr: Option<SocketAddr> = None;
        if let Some(mut adapter) = self.grpc_adapter.take() {
            let grpc_resolver = GrpcServiceResolver::new(self.routes_resolver.container.clone());
            let grpc_services = grpc_resolver.resolve()?;
            adapter
                .register_services(grpc_services)
                .map_err(|source| BindError::Adapter {
                    transport: "grpc",
                    source: source.into(),
                })?;
            let handle = adapter
                .into_lifecycle()
                .await
                .map_err(|source| BindError::Adapter {
                    transport: "grpc",
                    source: source.into(),
                })?;
            grpc_addr = handle.local_addr();
            if let Some(addr) = grpc_addr {
                tracing::info!(addr = %addr, "gRPC listening");
            }
            self.servers.push(Box::new(handle));
        }

        let http_addr = if let Some(http_adapter) = self.http_adapter.take() {
            let target = self.http_target.take().unwrap();
            let has_same_port_ws = !same_port.is_empty();
            let server_type = if has_same_port_ws {
                "HTTP + WebSocket"
            } else {
                "HTTP"
            };

            let ctx = AdapterContext::new(self.routes_resolver.take_global_chain());
            let handle = http_adapter
                .into_lifecycle(target, ctx)
                .await
                .map_err(|source| BindError::Adapter {
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
            return Err(anyhow::anyhow!(
                "No adapters configured; register at least one adapter before calling bind()"
            )
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
    /// Convenience wrapper over [`bind`](ToniApplication::bind) +
    /// [`run`](ToniApplication::run). Use this when you don't need the bound
    /// address; use `bind()` + `run()` explicitly when you do (dynamic ports,
    /// tests, readiness probes).
    pub async fn start(mut self) -> Result<()> {
        self.bind().await?;
        self.run().await;
        Ok(())
    }

    /// Drive the serve loops until all adapters exit or [`ShutdownHandle::shutdown`] is called.
    ///
    /// Consumes `self`. On a shutdown signal, adapters are closed first so
    /// in-flight requests can drain before lifecycle hooks run.
    ///
    /// # Panics
    ///
    /// Panics if called before [`bind`](ToniApplication::bind). Use
    /// [`start`](ToniApplication::start) to bind and run in one step.
    pub async fn run(mut self) {
        let bound = self
            .bound
            .take()
            .expect("run() called before bind() — call bind() first or use start()");

        let serve_all = Box::pin(futures::future::join_all(bound.serve_futures));
        let shutdown = self.shutdown.clone();
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
        self.call_module_destroy_hooks().await;
        self.call_before_shutdown_hooks(None).await;
        self.call_shutdown_hooks(None).await;
    }

    async fn close_adapters(&mut self) {
        if let Ok(bs) = self.get::<BroadcastService>().await {
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

    async fn call_before_shutdown_hooks(&self, signal: Option<String>) {
        self.context
            .call_before_shutdown_hooks(signal.clone())
            .await;
    }

    async fn call_module_destroy_hooks(&self) {
        self.context.call_module_destroy_hooks().await;
    }

    async fn call_shutdown_hooks(&self, signal: Option<String>) {
        self.context.call_shutdown_hooks(signal.clone()).await;
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
                    bs.connect(client_id.clone(), sink, gateway.get_namespace());
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
                    Ok(WsHandlerOutput::Empty) => MessageCallbackResult::Continue,
                    Ok(WsHandlerOutput::Single(response)) => {
                        handle.send_to(&client_id, response).await;
                        MessageCallbackResult::Continue
                    }
                    Ok(WsHandlerOutput::Stream(stream)) => MessageCallbackResult::Stream(stream),
                    Err(e) => match &e {
                        // Connection is already gone — stop the read loop.
                        WsError::ConnectionClosed(_) => MessageCallbackResult::Stop,
                        // Guard rejected this message; drop it silently and keep
                        // the connection alive so other handlers can still run.
                        WsError::AuthFailed(_) => MessageCallbackResult::Continue,
                        _ => {
                            let error_msg = WsMessage::text(
                                serde_json::json!({ "error": e.to_string() }).to_string(),
                            );
                            handle.send_to(&client_id, error_msg).await;
                            MessageCallbackResult::Continue
                        }
                    },
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
fn make_rpc_callbacks(wrappers: Vec<Arc<RpcControllerWrapper>>) -> RpcMessageCallbacks {
    let mut pattern_map: HashMap<String, Arc<RpcControllerWrapper>> = HashMap::new();
    for wrapper in &wrappers {
        for pattern in wrapper.get_patterns() {
            pattern_map.insert(pattern, wrapper.clone());
        }
    }
    let pattern_map = Arc::new(pattern_map);

    RpcMessageCallbacks::new(move |data: RpcData, ctx: RpcCallInfo| {
        let pattern_map = pattern_map.clone();
        Box::pin(async move {
            let pattern = ctx.pattern.clone();
            if let Some(wrapper) = pattern_map.get(&pattern) {
                wrapper.handle_message(data, ctx.headers, pattern).await
            } else {
                Err(RpcError::PatternNotFound(pattern))
            }
        })
    })
}
