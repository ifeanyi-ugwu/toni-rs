//! The standalone WebSocket server for gateways declared `port = own` (transports DESIGN §3.5),
//! as `ulo-http-hyper` is the HTTP server for `ulo-http`: `ulo-ws` knows the protocol, and this
//! crate the sockets. An HTTP/1.1 server over hyper and `ulo-hyper-serve`'s accept loop, on tokio,
//! which answers each upgrade request with `ulo-ws`'s handshake decision and hands each upgraded
//! connection to its driver, which runs it on the app's runtime.
//!
//! ```ignore
//! let app = App::builder(AppModule).runtime(ulo_tokio::Tokio::current()).wire()?.connect().await?
//!     .bind(ulo_ws_hyper::Server::new("0.0.0.0:9001"))
//!     .listen().await?;
//! ```

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;

use bytes::Bytes;
use futures_util::future::{self, Either};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use ulo::{Bound, BoundAddr, BoxError, DrainToken, Mounted, Transport};
use ulo_http::Upgraded;
use ulo_hyper_serve::{Accepted, Serve, ServeConfig};
pub use ulo_hyper_serve::ReadCount;
use ulo_net::rustls::ServerConfig;
use ulo_net::{Activation, ActivationError, Endpoint, EndpointSpec, ListenerName, Tls};
use ulo_transport::Count;
use ulo_transport::prepare::{Failures, zero_bound};
use ulo_ws::{GatewayDefaults, GatewayTable, Handshake, Ws};

/// The name a refusal in `prepare` gives the server.
const NAME: &str = "ulo_ws_hyper::Server";

/// 30 seconds, the HTTP server's own default for both connection clocks.
const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// The core's `Server` for [`Ws`] on its own port: `app.bind(ulo_ws_hyper::Server::new("0.0.0.0:9001"))`.
///
/// `prepare` builds the [`GatewayTable`] of the gateways declared `port = own`, refusing what the
/// table refuses, a server with no such gateway among it, and every zero limit; it resolves the
/// endpoints and loads TLS. Its failures are reported together, always in one order: the server's
/// two timeouts, what the table refuses, the endpoints, then TLS. `bind` binds every endpoint,
/// all-or-nothing. A request off the gateway paths is answered 404.
///
/// It serves the gateways declared `port = own`; the others are the HTTP server's, through the
/// hand-off `WsModule` registers. Rooms and broadcasts are the application's `WsModule`'s; an
/// application binding this server without importing `WsModule` gets an in-memory adapter of the
/// server's own, which no `Dep<Rooms>` reaches.
pub struct Server {
    endpoints: Vec<EndpointSpec>,
    tls: Option<Tls>,
    header_timeout: Bound,
    handshake_timeout: Bound,
    defaults: GatewayDefaults,
    read_count: ReadCount,
    prepared: Option<Prepared>,
    running: Option<Running>,
    bound: Vec<BoundAddr>,
}

/// What `prepare` built for `bind`.
struct Prepared {
    endpoints: Vec<Endpoint>,
    tls: Option<std::sync::Arc<ServerConfig>>,
    table: GatewayTable,
}

/// What `bind` built for `serve`, `drain` and `close`.
struct Running {
    serve: Serve,
    table: GatewayTable,
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
            defaults: GatewayDefaults::default(),
            read_count: ReadCount::default(),
            prepared: None,
            running: None,
            bound: Vec::new(),
        }
    }

    /// How many connections the server has read from, through a clone taken before the server
    /// moves into the app: each counted at its first read, after its TLS handshake on a `wss`
    /// endpoint.
    pub fn read_count(&self) -> ReadCount {
        self.read_count.clone()
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
        self.defaults = self.defaults.message_limit(bytes);
        self
    }

    pub fn max_connections(mut self, connections: Count) -> Self {
        self.defaults = self.defaults.max_connections(connections);
        self
    }

    /// Messages in flight per connection, over which the connection stops reading: 64 at
    /// `Count::Default`, none at `Count::Unlimited`.
    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.defaults = self.defaults.max_inflight(messages);
        self
    }

    /// Messages in flight across every connection of this server, over which every connection
    /// stops reading until a place frees: 1,024 at `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`),
    /// none at `Count::Unlimited`. As
    /// [`GatewayDefaults::server_max_inflight`](ulo_ws::GatewayDefaults::server_max_inflight).
    pub fn server_max_inflight(mut self, messages: Count) -> Self {
        self.defaults = self.defaults.server_max_inflight(messages);
        self
    }

    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.defaults = self.defaults.max_outbound(messages);
        self
    }

    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.defaults = self.defaults.ping_interval(interval);
        self
    }

    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.defaults = self.defaults.pong_timeout(timeout);
        self
    }
}

impl Server {
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
        failures.extend(zero_bound("header_timeout", self.header_timeout, "close every connection before its upgrade request"));
        failures.extend(zero_bound("handshake_timeout", self.handshake_timeout, "drop every TLS connection before its handshake"));
        let table = GatewayTable::own_port(NAME, &mounted, &self.defaults, &mut failures);
        let endpoints = self.resolve_endpoints(&mut failures);
        let tls = match &self.tls {
            None => None,
            Some(tls) => match tls.load(&[b"http/1.1".as_slice()]) {
                Ok(config) => Some(config),
                Err(error) => {
                    failures.push(error.to_string());
                    None
                }
            },
        };
        if !failures.is_empty() {
            return Err(Box::new(failures.into_error()));
        }
        self.prepared = Some(Prepared { endpoints, tls, table });
        Ok(())
    }

    async fn bind(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let _ = mounted;
        let Some(Prepared { endpoints, tls, table }) = self.prepared.take() else {
            return Err(BoxError::from("the WebSocket server was bound before it was prepared"));
        };
        let listeners = ulo_net::bind_all(&endpoints)?;
        let addrs: Vec<SocketAddr> = listeners.iter().map(ulo_net::BoundListener::local_addr).collect();
        let secure = tls.is_some();
        let config = ServeConfig { handshake_timeout: connection_timeout(self.handshake_timeout), read_count: self.read_count.clone() };
        let serve = Serve::new(listeners, tls, &config)?;
        self.bound = addrs.into_iter().map(|addr| BoundAddr::new(<Ws as Transport>::KEY, addr).tls(secure)).collect();
        table.start();
        let mut http1 = http1::Builder::new();
        http1.timer(TokioTimer::new()).header_read_timeout(connection_timeout(self.header_timeout));
        self.running = Some(Running { serve, table, http1 });
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(running) = &self.running else {
            return Err(BoxError::from("the WebSocket server was asked to serve before it was bound"));
        };
        let (table, http1) = (running.table.clone(), running.http1.clone());
        running.serve.run(move |accepted| connection(accepted, table.clone(), http1.clone())).await
    }

    /// The accept loop's drain beside the connections': idle ones close with 1001 at once, busy
    /// ones once their messages finish.
    async fn drain(&self, token: DrainToken) {
        if let Some(running) = &self.running {
            future::join(running.serve.drain(), running.table.drain(token)).await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        if let Some(running) = &self.running {
            future::join(running.serve.close(), running.table.close()).await;
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.bound.clone()
    }
}

/// One accepted connection: HTTP/1.1 with upgrades until it ends or upgrades, its graceful
/// shutdown started at the drain.
async fn connection(accepted: Accepted, table: GatewayTable, http1: http1::Builder) {
    let Accepted { io, conn, mut draining } = accepted;
    let peer = conn.peer;
    let service = hyper::service::service_fn(move |req: http::Request<Incoming>| {
        let table = table.clone();
        async move { Ok::<_, Infallible>(respond(&table, req, peer).await) }
    });
    let mut served = pin!(http1.serve_connection(TokioIo::new(io), service).with_upgrades());
    // The connection first: once it has ended, a graceful shutdown would have nothing to stop.
    let result = match future::select(served.as_mut(), pin!(draining.wait())).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => {
            served.as_mut().graceful_shutdown();
            served.await
        }
    };
    if let Err(error) = result {
        tracing::debug!(peer = ?peer, %error, "WebSocket server connection ended with an error");
    }
}

/// One request: the handshake decision written as hyper's response, and on a 101 the upgraded
/// connection handed to the driver.
async fn respond(table: &GatewayTable, mut req: http::Request<Incoming>, peer: Option<SocketAddr>) -> http::Response<Full<Bytes>> {
    let pending = hyper::upgrade::on(&mut req);
    let (head, _body) = req.into_parts();
    match table.handshake(head, peer).await {
        Handshake::Switch(switch) => {
            let response = switch.response().map(|()| Full::new(Bytes::new()));
            switch.serve(async move { pending.await.map(|io| Upgraded::from_tokio(TokioIo::new(io))).map_err(BoxError::from) });
            response
        }
        Handshake::Refuse(refusal) => refusal.into_response().map(|reason| Full::new(Bytes::from(reason))),
    }
}
