//! The gRPC server (transports DESIGN §6.2): HTTP/2 over `ulo-hyper-serve`, so endpoints, inherited
//! sockets and TLS have one story with HTTP's. tonic's own `transport::Server` is not used.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper::server::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tonic_health::ServingStatus;
use ulo::{Bound, BoundAddr, BoxError, DrainToken, Mounted, Transport, TypeName};
use ulo_http::stage::Stage;
use ulo_hyper_serve::{Accepted, Serve, ServeConfig};
use ulo_net::rustls::ServerConfig;
use ulo_net::{Activation, ActivationError, BoundListener, Endpoint, EndpointSpec, ListenerName, Tls};
use ulo_transport::prepare::{Failure, Failures, Names, zero_bound, zero_count};
use ulo_transport::{Admission, Count};

use crate::__private::GrpcHandler;
use crate::dispatch::{Dispatcher, Route, builtin};
use crate::health::GrpcHealth;
use crate::pre_dispatch::PreDispatch;
use crate::reflection;
use crate::transport::Grpc;

/// A TLS handshake's bound at `Bound::Default`, the HTTP server's.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// `timeout_grace` at `Bound::Default`, the HTTP server's.
const DEFAULT_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

/// The core's `Server` for [`Grpc`]: `app.bind(ulo_grpc::Server::new("0.0.0.0:50051"))`.
///
/// `prepare` builds the path table from the mounted handlers, refusing two handlers for one path,
/// builds the pre-dispatch stage from `PreDispatch<Grpc>`, refuses `Count::Max(0)` on each count
/// and `Bound::After(Duration::ZERO)` on `handshake_timeout` and `timeout_grace`, resolves the
/// endpoints and loads TLS
/// with ALPN `h2`. Health reports SERVING for every known service after `bind` and NOT_SERVING
/// from the drain on. Reflection, `grpc.reflection.v1` and `v1alpha`, is served in debug builds
/// and behind `reflection(true)` in release builds.
///
/// A gRPC method's path is its proto's, so a controller's `.at(prefix)` does not apply to it, and
/// `wire()` refuses a prefix on a controller whose handlers are all gRPC methods.
pub struct Server {
    pub(crate) endpoints: Vec<EndpointSpec>,
    pub(crate) tls: Option<Tls>,
    pub(crate) max_inflight: Count,
    pub(crate) max_per_connection: Count,
    pub(crate) max_concurrent_streams: Count,
    pub(crate) handshake_timeout: Bound,
    pub(crate) timeout_grace: Bound,
    pub(crate) reflection: bool,
    pub(crate) descriptor_sets: Vec<&'static [u8]>,
    /// Set by `prepare`, taken by `bind`.
    pub(crate) prepared: Option<Prepared>,
    /// Set by `bind`.
    pub(crate) running: Option<Running>,
    pub(crate) bound: Vec<BoundAddr>,
}

/// What `prepare` built for `bind`.
pub(crate) struct Prepared {
    endpoints: Vec<Endpoint>,
    tls: Option<Arc<ServerConfig>>,
    dispatcher: Arc<Dispatcher>,
    health: GrpcHealth,
    /// Every service a handler's path names, which health reports.
    services: Vec<String>,
}

/// What `bind` built for `serve`, `drain` and `close`.
pub(crate) struct Running {
    serve: Serve,
    dispatcher: Arc<Dispatcher>,
    connections: Arc<http2::Builder<TokioExecutor>>,
    health: GrpcHealth,
    services: Vec<String>,
}

impl Server {
    /// A server on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Server {
            endpoints: vec![endpoint.into()],
            tls: None,
            max_inflight: Count::Default,
            max_per_connection: Count::Default,
            max_concurrent_streams: Count::Default,
            handshake_timeout: Bound::Default,
            timeout_grace: Bound::Default,
            reflection: cfg!(debug_assertions),
            descriptor_sets: Vec::new(),
            prepared: None,
            running: None,
            bound: Vec::new(),
        }
    }

    /// One more endpoint the same server listens on.
    pub fn endpoint(mut self, endpoint: impl Into<EndpointSpec>) -> Self {
        self.endpoints.push(endpoint.into());
        self
    }

    /// TLS on every endpoint, ALPN `h2`, loaded in `prepare`.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Calls in flight across the server: 1,024 at `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`),
    /// none at `Count::Unlimited`; over it a call is answered UNAVAILABLE, which clients treat as
    /// retryable. `Count::Max(0)` is refused.
    pub fn max_inflight(mut self, calls: Count) -> Self {
        self.max_inflight = calls;
        self
    }

    /// Calls in flight per connection, refused UNAVAILABLE over it: no bound of its own at
    /// `Count::Default`, the server's `max_inflight` alone. `Count::Max(0)` is refused.
    pub fn max_per_connection(mut self, calls: Count) -> Self {
        self.max_per_connection = calls;
        self
    }

    /// `SETTINGS_MAX_CONCURRENT_STREAMS` per connection, excess streams refused with
    /// `REFUSED_STREAM`: hyper's own at `Count::Default`. `Count::Max(0)` is refused.
    pub fn max_concurrent_streams(mut self, streams: Count) -> Self {
        self.max_concurrent_streams = streams;
        self
    }

    /// How long a TLS handshake may take: 30 seconds at `Bound::Default`.
    /// `Bound::After(Duration::ZERO)` is refused in `prepare`.
    pub fn handshake_timeout(mut self, timeout: Bound) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// How long the error handlers may take with the `Timeout` a passed `grpc-timeout` offers them
    /// before DEADLINE_EXCEEDED is sent instead: one second at `Bound::Default`, timed by the
    /// app's `Timer`; `Bound::Unbounded` waits for them. `Bound::After(Duration::ZERO)` is refused
    /// in `prepare`.
    pub fn timeout_grace(mut self, grace: Bound) -> Self {
        self.timeout_grace = grace;
        self
    }

    /// Serves reflection in a release build; on in debug builds unset.
    pub fn reflection(mut self, enabled: bool) -> Self {
        self.reflection = enabled;
        self
    }

    /// An encoded file descriptor set reflection serves, as `include_proto!` exposes it:
    /// `.file_descriptor_set(pb::FILE_DESCRIPTOR_SET)`.
    pub fn file_descriptor_set(mut self, encoded: &'static [u8]) -> Self {
        self.descriptor_sets.push(encoded);
        self
    }

    /// Each count set to `Count::Max(0)` and each timeout set to `Bound::After(Duration::ZERO)`,
    /// which would refuse everything it governs.
    fn check_zeros(&self, failures: &mut Failures) {
        failures.extend(zero_count("max_inflight", self.max_inflight, "shed every call"));
        failures.extend(zero_count("max_per_connection", self.max_per_connection, "shed every call on every connection"));
        failures.extend(zero_count("max_concurrent_streams", self.max_concurrent_streams, "let no HTTP/2 stream open"));
        failures.extend(zero_bound("handshake_timeout", self.handshake_timeout, "drop every TLS connection before its handshake"));
        failures.extend(zero_bound(
            "timeout_grace",
            self.timeout_grace,
            "send DEADLINE_EXCEEDED before any error handler could answer a passed deadline",
        ));
    }

    /// Every endpoint resolved, an inherited one checked against the sockets this process
    /// inherited. Failures go to `failures`; the endpoints that resolved are returned.
    fn resolve_endpoints(&self, failures: &mut Failures) -> Vec<Endpoint> {
        let mut endpoints: Vec<Endpoint> = Vec::new();
        for spec in &self.endpoints {
            let endpoint = match spec.resolve() {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    failures.push(error.to_string());
                    continue;
                }
            };
            // Two port-0 endpoints are two listeners, each on a port of the OS's choosing.
            if matches!(&endpoint, Endpoint::Addr(addr) if addr.port() != 0) && endpoints.contains(&endpoint) {
                failures.push(format!("{endpoint} is listed twice"));
            }
            endpoints.push(endpoint);
        }
        check_inherited(&endpoints, failures);
        endpoints
    }
}

/// Each inherited name `endpoints` lists, against the sockets this process inherited: a name
/// listed more often than descriptors answer to it is refused, once per name.
fn check_inherited(endpoints: &[Endpoint], failures: &mut Failures) {
    let mut names: Vec<(&ListenerName, usize)> = Vec::new();
    for endpoint in endpoints {
        if let Endpoint::Inherited(name) = endpoint {
            match names.iter_mut().find(|(seen, _)| *seen == name) {
                Some((_, listed)) => *listed += 1,
                None => names.push((name, 1)),
            }
        }
    }
    if names.is_empty() {
        return;
    }
    // Read only when an endpoint is inherited: a process spawned by a socket-activated one
    // inherits a `LISTEN_PID` that is not its own and would fail here though it binds addresses
    // only.
    let activation = match Activation::get() {
        Ok(activation) => activation,
        Err(error) => {
            failures.push(error.to_string());
            return;
        }
    };
    for (name, listed) in names {
        match activation.count(name) {
            0 => failures.push(ActivationError::Missing(name.clone()).to_string()),
            inherited if listed > inherited => failures.push(format!(
                "{} is listed {listed} times, and {inherited} inherited sockets answer to it",
                Endpoint::Inherited(name.clone())
            )),
            _ => {}
        }
    }
}

/// A handler as a path-table failure names it, `` `UsersGrpc::get_user` ``.
#[derive(Clone, Copy)]
struct Who {
    controller: TypeName,
    name: &'static str,
}

impl Who {
    fn text(&self, names: &Names<'_>) -> String {
        format!("`{}::{}`", names.of(self.controller), self.name)
    }
}

/// The path table: one route per marker path, two handlers for one path refused naming both.
fn routes(mounted: &Mounted<'_, Grpc>, stage: &Stage<Grpc>, failures: &mut Failures) -> HashMap<&'static str, Arc<Route>> {
    let mut routes: HashMap<&'static str, (Arc<Route>, Who)> = HashMap::new();
    for handler in mounted.handlers() {
        let who = Who { controller: handler.controller().key().type_name(), name: handler.name() };
        let Some(grpc) = handler.handler::<GrpcHandler>() else {
            failures.push(Failure::naming(vec![who.controller], move |names| {
                format!("{} is mounted for gRPC without a gRPC handler value", who.text(names))
            }));
            continue;
        };
        if let Some((_, other)) = routes.get(grpc.path) {
            let (other, path) = (*other, grpc.path);
            failures.push(Failure::naming(vec![other.controller, who.controller], move |names| {
                format!("{} and {} both answer `{path}`", other.text(names), who.text(names))
            }));
            continue;
        }
        let scoped = match stage.scoped(grpc.path) {
            Ok(scoped) => scoped,
            Err(reason) => {
                failures.push(Failure::naming(vec![who.controller], move |names| format!("{}: {reason}", who.text(names))));
                continue;
            }
        };
        let route = Route { handler: handler.clone(), call: Arc::clone(&grpc.call), stage: scoped };
        routes.insert(grpc.path, (Arc::new(route), who));
    }
    routes.into_iter().map(|(path, (route, _))| (path, route)).collect()
}

/// The service a method path names, `users.v1.UserService` for `/users.v1.UserService/GetUser`.
fn service_name(path: &str) -> Option<&str> {
    path.strip_prefix('/')?.rsplit_once('/').map(|(service, _)| service)
}

fn limit(count: Count) -> Option<usize> {
    match count {
        Count::Max(max) => Some(max as usize),
        _ => None,
    }
}

impl ulo::Server for Server {
    type Transport = Grpc;

    async fn prepare(&mut self, mounted: Mounted<'_, Grpc>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        self.check_zeros(&mut failures);
        let stage = match Stage::build(mounted.module_meta::<PreDispatch>()) {
            Ok(stage) => Some(stage),
            Err(errors) => {
                failures.extend(errors);
                None
            }
        };
        let routes = match &stage {
            Some(stage) => routes(&mounted, stage, &mut failures),
            None => HashMap::new(),
        };
        let health = match mounted.app().get::<GrpcHealth>().await {
            Ok(health) => (*health).clone(),
            Err(_) => GrpcHealth::new(),
        };
        let mut builtins = vec![builtin(health.service())];
        if self.reflection {
            match reflection::services(&self.descriptor_sets) {
                Ok(services) => builtins.extend(services),
                Err(reason) => failures.push(reason),
            }
        }
        let endpoints = self.resolve_endpoints(&mut failures);
        let tls = match &self.tls {
            None => None,
            Some(tls) => match tls.load(&[b"h2".as_slice()]) {
                Ok(config) => Some(config),
                Err(error) => {
                    failures.push(error.to_string());
                    None
                }
            },
        };
        let Some(stage) = stage.filter(|_| failures.is_empty()) else {
            return failures.into_result();
        };
        let mut services: Vec<String> = routes.keys().filter_map(|path| service_name(path)).map(str::to_owned).collect();
        services.sort();
        services.dedup();
        let dispatcher = Dispatcher {
            app: mounted.app().clone(),
            timer: Arc::clone(mounted.timer()),
            routes,
            builtins,
            stage,
            admission: Admission::new(self.max_inflight.max_inflight()),
            per_connection: limit(self.max_per_connection),
            grace: match self.timeout_grace {
                Bound::Default => Some(DEFAULT_TIMEOUT_GRACE),
                Bound::After(grace) => Some(grace),
                Bound::Unbounded => None,
            },
        };
        self.prepared = Some(Prepared { endpoints, tls, dispatcher: Arc::new(dispatcher), health, services });
        Ok(())
    }

    async fn bind(&mut self, mounted: Mounted<'_, Grpc>) -> Result<(), BoxError> {
        let _ = mounted;
        let Some(Prepared { endpoints, tls, dispatcher, health, services }) = self.prepared.take() else {
            return Err(BoxError::from("the gRPC server was bound before it was prepared"));
        };
        let listeners = ulo_net::bind_all(&endpoints)?;
        let addrs: Vec<SocketAddr> = listeners.iter().map(BoundListener::local_addr).collect();
        let secure = tls.is_some();
        let handshake_timeout = match self.handshake_timeout {
            Bound::Default => Some(DEFAULT_HANDSHAKE_TIMEOUT),
            Bound::After(after) => Some(after),
            Bound::Unbounded => None,
        };
        let serve = Serve::new(listeners, tls, &ServeConfig { handshake_timeout })?;
        let mut connections = http2::Builder::new(TokioExecutor::new());
        connections.timer(TokioTimer::new());
        match self.max_concurrent_streams {
            Count::Default => {}
            Count::Max(streams) => {
                connections.max_concurrent_streams(streams);
            }
            Count::Unlimited => {
                connections.max_concurrent_streams(None);
            }
        }
        self.bound = addrs.into_iter().map(|addr| BoundAddr::new(<Grpc as Transport>::KEY, addr).tls(secure)).collect();
        for service in &services {
            health.set_named(service, ServingStatus::Serving).await;
        }
        self.running = Some(Running { serve, dispatcher, connections: Arc::new(connections), health, services });
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(running) = &self.running else {
            return Err("the gRPC server was asked to serve before it was bound".into());
        };
        let dispatcher = Arc::clone(&running.dispatcher);
        let connections = Arc::clone(&running.connections);
        running.serve.run(move |accepted| connection(accepted, Arc::clone(&dispatcher), Arc::clone(&connections))).await
    }

    /// GOAWAY through `ulo-hyper-serve`'s drain; health switches to NOT_SERVING first, the overall
    /// status `""` included, so a client polling health stops sending before the GOAWAY arrives.
    async fn drain(&self, token: DrainToken) {
        let _ = token;
        let Some(running) = &self.running else { return };
        running.health.set_named("", ServingStatus::NotServing).await;
        for service in &running.services {
            running.health.set_named(service, ServingStatus::NotServing).await;
        }
        running.serve.drain().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        if let Some(running) = &self.running {
            running.serve.close().await;
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.bound.clone()
    }
}

/// One connection after its handshake: HTTP/2 until it ends, each call answered by the
/// dispatcher, the connection's graceful shutdown, GOAWAY, started once the drain begins.
async fn connection(accepted: Accepted, dispatcher: Arc<Dispatcher>, connections: Arc<http2::Builder<TokioExecutor>>) {
    let Accepted { io, conn, mut draining } = accepted;
    let peer = conn.peer;
    let admission = dispatcher.connection();
    let service = hyper::service::service_fn(move |req: http::Request<Incoming>| {
        let reply = Arc::clone(&dispatcher).call(req, conn.clone(), admission.clone());
        async move { Ok::<_, Infallible>(reply.await) }
    });
    let mut serving = std::pin::pin!(connections.serve_connection(TokioIo::new(io), service));
    let mut shutting_down = false;
    loop {
        tokio::select! {
            result = serving.as_mut() => {
                if let Err(error) = result {
                    tracing::debug!(peer = ?peer, %error, "gRPC connection ended with an error");
                }
                break;
            }
            () = draining.wait(), if !shutting_down => {
                shutting_down = true;
                serving.as_mut().graceful_shutdown();
            }
        }
    }
}
