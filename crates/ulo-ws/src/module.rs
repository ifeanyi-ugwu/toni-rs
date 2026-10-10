//! `WsModule`: what a WebSocket application imports once (transports DESIGN §4.1).

use std::sync::Arc;

use bytes::Bytes;
use futures_core::stream::BoxStream;
use ulo::{BoxError, BoxFuture, Bound, Meta, Module, ModuleDef, ModuleIdentity};
use ulo_http::Upgrades;
use ulo_transport::Count;

use crate::broadcast::{BroadcastAdapter, InMemory, NodeId, Target};
use crate::handoff::Handoff;
use crate::rooms::{Hub, Rooms};
use crate::table::GatewayDefaults;

/// Imported once per application: registers the gateway hand-off in the HTTP `Upgrades`
/// metadata, binds [`Rooms`](crate::Rooms) and the broadcast adapter, and carries the defaults
/// for the gateways on the HTTP server's port that a standalone server, `ulo_ws_hyper::Server`,
/// carries for those on its own port.
///
/// ```ignore
/// #[module(imports = [ulo_ws::WsModule::for_root().broadcast(ulo_ws_redis::Redis::url("redis://cache:6379"))])]
/// pub struct AppModule;
/// ```
///
/// A gateway's mount records handlers and cannot bind, which is why the module exists: without it
/// a gateway on the HTTP server's port is reachable by nothing and `Dep<Rooms>` is a missing
/// dependency at `wire()`.
///
/// On the HTTP port, the HTTP server's drain and load shedding answer first: an upgrade request
/// arriving once the drain has begun, or over the HTTP server's `max_inflight`, gets the HTTP
/// server's 503 before the hand-off sees it.
#[derive(Clone)]
pub struct WsModule {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    pub(crate) defaults: GatewayDefaults,
}

impl WsModule {
    /// The in-memory broadcast adapter and every default unset.
    pub fn for_root() -> Self {
        WsModule { adapter: Arc::new(InMemory::new()), defaults: GatewayDefaults::default() }
    }

    /// The broadcast adapter in place of the in-memory one, as configuration: a second module
    /// binding `dyn BroadcastAdapter` would be `DuplicateBinding` at `wire()`.
    pub fn broadcast(mut self, adapter: impl BroadcastAdapter) -> Self {
        self.adapter = Arc::new(adapter);
        self
    }

    /// Bytes per message after reassembly: 64 MiB at the default. Zero is refused in `prepare`.
    pub fn message_limit(mut self, bytes: u64) -> Self {
        self.defaults.message_limit = Some(bytes);
        self
    }

    /// Connections per gateway: unbounded at `Count::Default`. Over it a connection is accepted
    /// and closed with 1013. `Count::Max(0)` is refused in `prepare`.
    pub fn max_connections(mut self, connections: Count) -> Self {
        self.defaults.max_connections = connections;
        self
    }

    /// Messages in flight per connection, over which the connection stops reading: 1,024 at
    /// `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`), none at `Count::Unlimited`.
    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.defaults.max_inflight = messages;
        self
    }

    /// Queued outbound messages per connection, over which the gateway's `overflow` applies.
    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.defaults.max_outbound = messages;
        self
    }

    /// The keep-alive Ping's period: 30 seconds at `Bound::Default`; `Bound::Unbounded` sends none.
    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.defaults.ping_interval = interval;
        self
    }

    /// How long a Pong may take before the connection ends as `Lost`: 30 seconds at
    /// `Bound::Default`.
    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.defaults.pong_timeout = timeout;
        self
    }
}

impl Module for WsModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<WsModule>()
    }

    /// The module is global, so `Dep<Rooms>` and `Dep<dyn BroadcastAdapter>` resolve in every
    /// module of the application, and the standalone server finds the application's `Hub` in its
    /// metadata whichever module imported it.
    fn register(&self, m: &mut ModuleDef<'_>) {
        let hub = Arc::new(Hub::new(Arc::clone(&self.adapter)));
        m.global();
        m.meta::<Upgrades>().register(Handoff::new(self.defaults.clone(), Arc::clone(&hub)));
        m.meta::<HubMeta>().hub = Some(Arc::clone(&hub));
        m.value(Rooms { adapter: Arc::clone(&self.adapter), hub });
        m.export::<Rooms>();
        m.value(SharedAdapter(Arc::clone(&self.adapter))).also_as::<dyn BroadcastAdapter>(|adapter| adapter);
        m.export::<dyn BroadcastAdapter>();
    }
}

/// The application's `Hub`, as module metadata the standalone server reads in `prepare`.
#[derive(Default)]
pub(crate) struct HubMeta {
    pub(crate) hub: Option<Arc<Hub>>,
}

impl Meta for HubMeta {}

/// The configured adapter under a key of its own, bound `also_as` `dyn BroadcastAdapter`.
struct SharedAdapter(Arc<dyn BroadcastAdapter>);

impl BroadcastAdapter for SharedAdapter {
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        self.0.publish(target, frame)
    }

    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        self.0.subscribe(node)
    }
}
