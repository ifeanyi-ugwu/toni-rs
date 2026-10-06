//! The standalone WebSocket server for gateways on their own port (transports DESIGN §3.5): an
//! HTTP/1.1 server over `ulo-hyper-serve` with upgrades, answering 404 off the gateway paths and
//! the 101 on them.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_TYPE, HeaderValue};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use ulo::{AppHandle, Bound, BoundAddr, BoxError, DrainToken, Mounted, Timer, Transport};
use ulo_hyper_serve::{Accepted, Serve, ServeConfig};
use ulo_net::{Activation, ActivationError, Endpoint, EndpointSpec, ListenerName, Tls, TlsAcceptor};
use ulo_transport::Count;
use ulo_transport::prepare::{Failures, zero_bound};

use crate::broadcast::InMemory;
use crate::connection::{Accept, Answer, Tracker, answer};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults, same_path};
use crate::handoff::after_init;
use crate::module::{Defaults, HubMeta};
use crate::rooms::Hub;
use crate::transport::{Ws, WsConnect};

/// 30 seconds, the HTTP server's own default for both connection clocks.
const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// The core's `Server` for [`Ws`], on its own port: `app.bind(ulo_ws::Server::new("0.0.0.0:9001"))`.
///
/// `prepare` pairs the message handlers with the connect handlers mounted under `WsConnect`
/// (`Mounted::handlers_of`, X20) by controller, refuses two gateways on one path and every zero
/// limit, resolves the endpoints and loads TLS; `bind` binds every endpoint, all-or-nothing.
///
/// It serves the gateways declared `port = own`; the others are the HTTP server's, through the
/// hand-off `WsModule` registers. A server with no such gateway is refused in `prepare`. Rooms
/// and broadcasts are the application's `WsModule`'s; an application binding this server without
/// importing `WsModule` gets an in-memory adapter of the server's own, which no `Dep<Rooms>`
/// reaches.
pub struct Server {
    pub(crate) endpoints: Vec<EndpointSpec>,
    pub(crate) tls: Option<Tls>,
    pub(crate) header_timeout: Bound,
    pub(crate) handshake_timeout: Bound,
    pub(crate) message_limit: Option<u64>,
    pub(crate) max_connections: Count,
    pub(crate) max_inflight: Count,
    pub(crate) max_outbound: Count,
    pub(crate) ping_interval: Bound,
    pub(crate) pong_timeout: Bound,
    pub(crate) prepared: Option<Prepared>,
    pub(crate) running: Option<Running>,
    pub(crate) tracker: Arc<Tracker>,
    pub(crate) bound: Vec<BoundAddr>,
}

/// What `prepare` built for `bind`.
pub(crate) struct Prepared {
    endpoints: Vec<Endpoint>,
    tls: Option<TlsAcceptor>,
    routes: Arc<Routes>,
}

/// What `bind` built for `serve`, `drain` and `close`.
pub(crate) struct Running {
    serve: Serve,
    routes: Arc<Routes>,
}

/// Everything one request reads.
pub(crate) struct Routes {
    gateways: Vec<Arc<GatewayRuntime>>,
    hub: Arc<Hub>,
    app: AppHandle,
    timer: Arc<dyn Timer>,
    tracker: Arc<Tracker>,
    http1: http1::Builder,
}

impl Server {
    /// A server on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Server {
            endpoints: vec![endpoint.into()],
            tls: None,
            header_timeout: Bound::Default,
            handshake_timeout: Bound::Default,
            message_limit: None,
            max_connections: Count::Default,
            max_inflight: Count::Default,
            max_outbound: Count::Default,
            ping_interval: Bound::Default,
            pong_timeout: Bound::Default,
            prepared: None,
            running: None,
            tracker: Tracker::new(),
            bound: Vec::new(),
        }
    }

    /// One more endpoint the same server listens on.
    pub fn endpoint(mut self, endpoint: impl Into<EndpointSpec>) -> Self {
        self.endpoints.push(endpoint.into());
        self
    }

    /// `wss`: TLS on every endpoint, loaded in `prepare`.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// How long the upgrade request's head may take: 30 seconds at `Bound::Default`.
    /// `Bound::After(Duration::ZERO)` is refused in `prepare`.
    pub fn header_timeout(mut self, timeout: Bound) -> Self {
        self.header_timeout = timeout;
        self
    }

    /// How long a TLS handshake may take: 30 seconds at `Bound::Default`.
    /// `Bound::After(Duration::ZERO)` is refused in `prepare`.
    pub fn handshake_timeout(mut self, timeout: Bound) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// Bytes per message after reassembly: 64 MiB at the default. Zero is refused in `prepare`.
    pub fn message_limit(mut self, bytes: u64) -> Self {
        self.message_limit = Some(bytes);
        self
    }

    pub fn max_connections(mut self, connections: Count) -> Self {
        self.max_connections = connections;
        self
    }

    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.max_inflight = messages;
        self
    }

    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.max_outbound = messages;
        self
    }

    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.ping_interval = interval;
        self
    }

    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.pong_timeout = timeout;
        self
    }
}

impl Server {
    fn defaults(&self) -> Defaults {
        Defaults {
            message_limit: self.message_limit,
            max_connections: self.max_connections,
            max_inflight: self.max_inflight,
            max_outbound: self.max_outbound,
            ping_interval: self.ping_interval,
            pong_timeout: self.pong_timeout,
        }
    }

    /// Every endpoint resolved, an inherited one checked against the sockets this process
    /// inherited.
    fn resolve_endpoints(&self, failures: &mut Failures) -> Vec<Endpoint> {
        let mut endpoints: Vec<Endpoint> = Vec::new();
        for spec in &self.endpoints {
            match spec.resolve() {
                Ok(endpoint) => {
                    if matches!(endpoint, Endpoint::Addr(addr) if addr.port() != 0) && endpoints.contains(&endpoint) {
                        failures.push(format!("{endpoint} is listed twice"));
                    }
                    endpoints.push(endpoint);
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        check_inherited(&endpoints, failures);
        endpoints
    }
}

/// Each inherited name `endpoints` lists, against the sockets this process inherited, as the
/// HTTP server checks them; the environment is read only when an endpoint is inherited.
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

fn connection_timeout(bound: Bound) -> Option<Duration> {
    match bound {
        Bound::Default => Some(DEFAULT_CONNECTION_TIMEOUT),
        Bound::After(after) => Some(after),
        Bound::Unbounded => None,
    }
}

impl ulo::Server for Server {
    type Transport = Ws;

    async fn prepare(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        let defaults = self.defaults();
        check_defaults("ulo_ws::Server", &defaults, &mut failures);
        failures.extend(zero_bound("header_timeout", self.header_timeout, "close every connection before its upgrade request"));
        failures.extend(zero_bound("handshake_timeout", self.handshake_timeout, "drop every TLS connection before its handshake"));
        let connects = mounted.handlers_of::<WsConnect>();
        let gateways = build_table(&connects, mounted.handlers(), Port::Own, &defaults, &mut failures);
        if gateways.is_empty() {
            failures.push(
                "`ulo_ws::Server` serves the gateways declared `port = own`, and none is; a gateway without it is served on the \
                 HTTP server's port",
            );
        }
        let endpoints = self.resolve_endpoints(&mut failures);
        let tls = match &self.tls {
            None => None,
            Some(tls) => match tls.load(&[b"http/1.1".as_slice()]) {
                Ok(acceptor) => Some(acceptor),
                Err(error) => {
                    failures.push(error.to_string());
                    None
                }
            },
        };
        if !failures.is_empty() {
            return Err(Box::new(failures.into_error()));
        }
        let hub = mounted
            .module_meta::<HubMeta>()
            .into_iter()
            .find_map(|(_, meta)| meta.hub.clone())
            .unwrap_or_else(|| Arc::new(Hub::new(Arc::new(InMemory::new()))));
        hub.record_gateways(&gateways);
        let mut http1 = http1::Builder::new();
        http1.timer(TokioTimer::new()).header_read_timeout(connection_timeout(self.header_timeout));
        let routes = Arc::new(Routes {
            gateways,
            hub,
            app: mounted.app().clone(),
            timer: Arc::clone(mounted.timer()),
            tracker: Arc::clone(&self.tracker),
            http1,
        });
        self.prepared = Some(Prepared { endpoints, tls, routes });
        Ok(())
    }

    async fn bind(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let _ = mounted;
        let Some(Prepared { endpoints, tls, routes }) = self.prepared.take() else {
            return Err(BoxError::from("the WebSocket server was bound before it was prepared"));
        };
        let listeners = ulo_net::bind_all(&endpoints)?;
        let addrs: Vec<SocketAddr> = listeners.iter().map(ulo_net::BoundListener::local_addr).collect();
        let secure = tls.is_some();
        let config = ServeConfig { handshake_timeout: connection_timeout(self.handshake_timeout) };
        let serve = Serve::new(listeners, tls, &config)?;
        self.bound = addrs.into_iter().map(|addr| BoundAddr::new(<Ws as Transport>::KEY, addr).tls(secure)).collect();
        routes.hub.start();
        after_init(&routes.gateways);
        self.running = Some(Running { serve, routes });
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(running) = &self.running else {
            return Err(BoxError::from("the WebSocket server was asked to serve before it was bound"));
        };
        let routes = Arc::clone(&running.routes);
        running.serve.run(move |accepted| connection(accepted, Arc::clone(&routes))).await
    }

    /// The accept loop's drain beside the connections': idle ones close with 1001 at once, busy
    /// ones once their messages finish.
    async fn drain(&self, token: DrainToken) {
        match &self.running {
            Some(running) => {
                futures_util::future::join(running.serve.drain(), self.tracker.drain(token)).await;
            }
            None => self.tracker.drain(token).await,
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        match &self.running {
            Some(running) => {
                futures_util::future::join(running.serve.close(), self.tracker.close()).await;
                running.routes.hub.stop();
            }
            None => self.tracker.close().await,
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.bound.clone()
    }
}

/// One accepted connection: HTTP/1.1 with upgrades until it ends or upgrades, its graceful
/// shutdown started at the drain.
async fn connection(accepted: Accepted, routes: Arc<Routes>) {
    let Accepted { io, conn, mut draining } = accepted;
    let peer = conn.peer;
    let service_routes = Arc::clone(&routes);
    let service = hyper::service::service_fn(move |req: http::Request<Incoming>| {
        let routes = Arc::clone(&service_routes);
        async move { Ok::<_, Infallible>(routes.respond(req, peer).await) }
    });
    let mut served = std::pin::pin!(routes.http1.serve_connection(TokioIo::new(io), service).with_upgrades());
    let mut shutting_down = false;
    loop {
        tokio::select! {
            result = served.as_mut() => {
                if let Err(error) = result {
                    tracing::debug!(peer = ?peer, %error, "WebSocket server connection ended with an error");
                }
                break;
            }
            () = draining.wait(), if !shutting_down => {
                shutting_down = true;
                served.as_mut().graceful_shutdown();
            }
        }
    }
}

impl Routes {
    async fn respond(&self, mut req: http::Request<Incoming>, peer: Option<SocketAddr>) -> http::Response<Full<Bytes>> {
        let Some(gateway) = self.gateways.iter().find(|gateway| same_path(&gateway.path, req.uri().path())).cloned() else {
            return plain(StatusCode::NOT_FOUND, "no gateway at this path");
        };
        let pending = hyper::upgrade::on(&mut req);
        let (head, _body) = req.into_parts();
        let upgraded = async move { pending.await.map(TokioIo::new).map_err(BoxError::from) };
        let accept = Accept {
            gateway,
            hub: Arc::clone(&self.hub),
            app: self.app.clone(),
            timer: Arc::clone(&self.timer),
            tracker: Arc::clone(&self.tracker),
        };
        match answer(accept, head, peer, upgraded).await {
            Answer::Switch(headers) => {
                let mut response = http::Response::new(Full::new(Bytes::new()));
                *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                response.headers_mut().extend(headers);
                response
            }
            Answer::Refuse { status, reason, headers } => {
                let mut response = plain(status, reason);
                for (name, value) in headers {
                    response.headers_mut().insert(name, value);
                }
                response
            }
        }
    }
}

fn plain(status: StatusCode, text: impl Into<String>) -> http::Response<Full<Bytes>> {
    let mut response = http::Response::new(Full::new(Bytes::from(text.into())));
    *response.status_mut() = status;
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}
