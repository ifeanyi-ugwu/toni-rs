//! The standalone WebSocket server for gateways on their own port (transports DESIGN §3.5): an
//! HTTP/1.1 server over `ulo-hyper-serve` with upgrades, answering 404 off the gateway paths and
//! the 101 on them.

use ulo::{Bound, BoundAddr, BoxError, DrainToken, Mounted};
use ulo_net::{EndpointSpec, Tls};
use ulo_transport::Count;

use crate::transport::Ws;

/// The core's `Server` for [`Ws`], on its own port: `app.bind(ulo_ws::Server::new("0.0.0.0:9001"))`.
///
/// `prepare` pairs the message handlers with the connect handlers mounted under `WsConnect`
/// (`Mounted::handlers_of`, X20) by controller, refuses two gateways on one path and every zero
/// limit, resolves the endpoints and loads TLS; `bind` binds every endpoint, all-or-nothing.
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

impl ulo::Server for Server {
    type Transport = Ws;

    async fn prepare(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn bind(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn serve(&self) -> Result<(), BoxError> {
        todo!()
    }

    async fn drain(&self, token: DrainToken) {
        let _ = token;
        todo!()
    }

    async fn close(&self) -> Result<(), BoxError> {
        todo!()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        todo!()
    }
}
