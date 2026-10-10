//! What a server serving gateways needs from the hub (transports DESIGN §3.5): the gateway table
//! it serves, the handshake decision for each upgrade request, and, through
//! [`Switch::serve`](crate::Switch::serve), the connection driver. The hand-off on the HTTP
//! server's port and `ulo-ws-hyper`'s standalone server both serve through it, so a server is the
//! socket code around these three calls.
//!
//! ```ignore
//! // `prepare`
//! let table = GatewayTable::own_port("my_server::Server", &mounted, &defaults, &mut failures);
//! // `bind`
//! table.start();
//! // each upgrade request
//! match table.handshake(head, peer).await {
//!     Handshake::Switch(switch) => {
//!         write(switch.response());
//!         switch.serve(async move { upgraded_io.await });
//!     }
//!     Handshake::Refuse(refusal) => write(refusal.into_response()),
//! }
//! // `drain`, `close`
//! table.drain(token).await;
//! table.close().await;
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use http::StatusCode;
use http::request::Parts;
use ulo::{AppHandle, Bound, DrainToken, Mounted, Runtime};
use ulo_transport::Count;
use ulo_transport::prepare::Failures;

use crate::broadcast::InMemory;
use crate::connection::{self, Accept, Handshake, Refusal, Tracker};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults, same_path};
use crate::handoff::after_init;
use crate::module::HubMeta;
use crate::rooms::Hub;
use crate::transport::{Ws, WsConnect};

/// The settings a gateway reads where its attribute leaves one unset: `WsModule`'s for the
/// gateways on the HTTP server's port, a standalone server's for those on its own.
#[derive(Clone, Debug, Default)]
pub struct GatewayDefaults {
    pub(crate) message_limit: Option<u64>,
    pub(crate) max_connections: Count,
    pub(crate) max_inflight: Count,
    pub(crate) max_outbound: Count,
    pub(crate) ping_interval: Bound,
    pub(crate) pong_timeout: Bound,
}

impl GatewayDefaults {
    /// Bytes per message after reassembly: 64 MiB at the default. Zero is refused in `prepare`.
    pub fn message_limit(mut self, bytes: u64) -> Self {
        self.message_limit = Some(bytes);
        self
    }

    /// Connections per gateway: unbounded at `Count::Default`. Over it a connection is accepted
    /// and closed with 1013. `Count::Max(0)` is refused in `prepare`.
    pub fn max_connections(mut self, connections: Count) -> Self {
        self.max_connections = connections;
        self
    }

    /// Messages in flight per connection, over which the connection stops reading: 1,024 at
    /// `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`), none at `Count::Unlimited`.
    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.max_inflight = messages;
        self
    }

    /// Queued outbound messages per connection, over which the gateway's `overflow` applies.
    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.max_outbound = messages;
        self
    }

    /// The keep-alive Ping's period: 30 seconds at `Bound::Default`; `Bound::Unbounded` sends none.
    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.ping_interval = interval;
        self
    }

    /// How long a Pong may take before the connection ends as `Lost`: 30 seconds at
    /// `Bound::Default`.
    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.pong_timeout = timeout;
        self
    }
}

/// The gateways one server serves, with the application's rooms, its runtime, and the
/// connections the server has open. Cheap to clone; every clone is the same table.
#[derive(Clone)]
pub struct GatewayTable {
    inner: Arc<Inner>,
}

struct Inner {
    gateways: Vec<Arc<GatewayRuntime>>,
    hub: Arc<Hub>,
    app: AppHandle,
    runtime: Arc<dyn Runtime>,
    tracker: Arc<Tracker>,
}

impl GatewayTable {
    /// The gateways declared `port = own`, for a standalone server's `prepare`: each connect
    /// handler `ulo-ws` mounted under `WsConnect` paired by controller with its message handlers,
    /// `defaults` filling what a gateway's attribute leaves unset.
    ///
    /// Pushes onto `failures`, naming `server`, every zero default, every gateway the table
    /// refuses (a path not starting with `/` or carrying `{param}`, two gateways on one path,
    /// reserved event names, two handlers for one event, every zero limit), and the absence of
    /// any `port = own` gateway. The table is built either way; a server reports `failures` and
    /// binds nothing when any was pushed.
    ///
    /// Rooms and broadcasts are the application's `WsModule`'s; an application without one gets
    /// an in-memory adapter of the table's own, which no `Dep<Rooms>` reaches.
    pub fn own_port(server: &str, mounted: &Mounted<'_, Ws>, defaults: &GatewayDefaults, failures: &mut Failures) -> GatewayTable {
        check_defaults(server, defaults, failures);
        let connects = mounted.handlers_of::<WsConnect>();
        let gateways = build_table(&connects, mounted.handlers(), Port::Own, defaults, failures);
        if gateways.is_empty() {
            failures.push(format!(
                "`{server}` serves the gateways declared `port = own`, and none is; a gateway without it is served on the HTTP \
                 server's port"
            ));
        }
        let hub = mounted
            .module_meta::<HubMeta>()
            .into_iter()
            .find_map(|(_, meta)| meta.hub.clone())
            .unwrap_or_else(|| Arc::new(Hub::new(Arc::new(InMemory::new()))));
        GatewayTable::new(gateways, hub, mounted.app().clone(), Arc::clone(mounted.runtime()))
    }

    pub(crate) fn new(gateways: Vec<Arc<GatewayRuntime>>, hub: Arc<Hub>, app: AppHandle, runtime: Arc<dyn Runtime>) -> Self {
        hub.record_gateways(&gateways);
        GatewayTable { inner: Arc::new(Inner { gateways, hub, app, runtime, tracker: Tracker::new() }) }
    }

    /// Each gateway's path, its controller's prefix joined.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.inner.gateways.iter().map(|gateway| &*gateway.path)
    }

    /// Starts the application's broadcast delivery, once whichever server starts it, and each
    /// gateway's `AfterInit`, each a task on the app's runtime. A server calls it once bound.
    pub fn start(&self) {
        self.inner.hub.start(&*self.inner.runtime);
        after_init(&self.inner.gateways, &*self.inner.runtime);
    }

    /// The decision on one upgrade request, made from its head alone: the gateway at its path
    /// (404 when none), RFC 6455's checks (405 for a method other than GET, 400 for HTTP/1.0, a
    /// missing `Connection: Upgrade`, `Upgrade: websocket` or `Sec-WebSocket-Key`, 426 for a
    /// version other than 13), 503 once the drain has begun (on the HTTP server's port that
    /// server's own drain answers first), and the first of the gateway's subprotocols the client
    /// offered. Under `refuse = handshake` the connection phase runs
    /// here, its connect guards and `OnConnect`, and a refusal is its 401 or 403.
    ///
    /// `peer` is the client's address, which `ConnectionInfo::peer` reports.
    pub async fn handshake(&self, head: Parts, peer: Option<SocketAddr>) -> Handshake {
        let Some(gateway) = self.gateway_for(head.uri.path()) else {
            return Handshake::Refuse(Refusal::new(StatusCode::NOT_FOUND, "no gateway at this path"));
        };
        connection::handshake(self.accept(gateway), head, peer).await
    }

    /// Raises the drain on this table's connections: idle ones close with 1001 at once, busy ones
    /// stop reading, finish their messages and then close. Returns once every connection has
    /// ended; a handshake from here on is refused 503.
    pub async fn drain(&self, token: DrainToken) {
        self.inner.tracker.drain(token).await;
    }

    /// Aborts every connection left, returns once their tasks have ended, and stops broadcast
    /// delivery.
    pub async fn close(&self) {
        self.inner.tracker.close().await;
        self.inner.hub.stop();
    }

    pub(crate) fn gateway_for(&self, path: &str) -> Option<Arc<GatewayRuntime>> {
        self.inner.gateways.iter().find(|gateway| same_path(&gateway.path, path)).cloned()
    }

    fn accept(&self, gateway: Arc<GatewayRuntime>) -> Accept {
        Accept {
            gateway,
            hub: Arc::clone(&self.inner.hub),
            app: self.inner.app.clone(),
            runtime: Arc::clone(&self.inner.runtime),
            tracker: Arc::clone(&self.inner.tracker),
        }
    }
}
