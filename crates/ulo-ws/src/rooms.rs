//! Rooms: addressing a gateway's connections from any code that holds `Dep<Rooms>` (transports
//! DESIGN §4.3).
//!
//! ```ignore
//! rooms.to_all().emit("notice", &n).await?;
//! rooms.gateway::<ChatGateway>().to_room("lobby").except([me]).emit("chat.message", &m).await?;
//! rooms.to_client(id).emit("dm", &dm).await?;
//! ```

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::StreamExt;
use serde::Serialize;
use ulo::{BoxError, TypeName};
use ulo_transport::{Classify, ErrorKind};

use crate::broadcast::{Audience, BroadcastAdapter, NodeId, Target};
use crate::connection::{ConnId, Outbound};
use crate::envelope;
use crate::gateway::{GatewayConfig, GatewayRuntime};

/// One application's connections in this process: who is connected on which gateway, the rooms
/// each has joined, and the delivery of every broadcast the adapter carries to the members here.
/// `WsModule` builds one per application and hands it to the hand-off, to the standalone server
/// through its metadata, and to `Rooms`.
pub(crate) struct Hub {
    pub(crate) node: NodeId,
    seq: AtomicU64,
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    members: Mutex<HashMap<ConnId, Member>>,
    /// Each prepared gateway's path under its controller's type, for `Rooms::gateway::<G>()`.
    paths: Mutex<HashMap<TypeName, Arc<str>>>,
    delivery: Mutex<Option<tokio::task::AbortHandle>>,
}

struct Member {
    gateway: Arc<GatewayRuntime>,
    rooms: HashSet<String>,
    outbound: Arc<Outbound>,
}

impl Hub {
    pub(crate) fn new(adapter: Arc<dyn BroadcastAdapter>) -> Self {
        Hub {
            node: NodeId::current(),
            seq: AtomicU64::new(0),
            adapter,
            members: Mutex::new(HashMap::new()),
            paths: Mutex::new(HashMap::new()),
            delivery: Mutex::new(None),
        }
    }

    pub(crate) fn next_id(&self) -> ConnId {
        ConnId { node: self.node, seq: self.seq.fetch_add(1, Ordering::Relaxed) }
    }

    pub(crate) fn record_gateways(&self, gateways: &[Arc<GatewayRuntime>]) {
        let mut paths = self.paths.lock().unwrap_or_else(PoisonError::into_inner);
        for gateway in gateways {
            paths.insert(gateway.controller, Arc::clone(&gateway.path));
        }
    }

    fn path_of(&self, controller: TypeName) -> Option<Arc<str>> {
        self.paths.lock().unwrap_or_else(PoisonError::into_inner).get(&controller).cloned()
    }

    pub(crate) fn register(&self, id: ConnId, gateway: Arc<GatewayRuntime>, outbound: Arc<Outbound>) {
        let member = Member { gateway, rooms: HashSet::new(), outbound };
        self.members.lock().unwrap_or_else(PoisonError::into_inner).insert(id, member);
    }

    pub(crate) fn unregister(&self, id: ConnId) {
        self.members.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }

    pub(crate) fn join(&self, id: ConnId, room: String) {
        if let Some(member) = self.members.lock().unwrap_or_else(PoisonError::into_inner).get_mut(&id) {
            member.rooms.insert(room);
        }
    }

    pub(crate) fn leave(&self, id: ConnId, room: &str) {
        if let Some(member) = self.members.lock().unwrap_or_else(PoisonError::into_inner).get_mut(&id) {
            member.rooms.remove(room);
        }
    }

    pub(crate) fn rooms(&self, id: ConnId) -> Vec<String> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        members.get(&id).map(|member| member.rooms.iter().cloned().collect()).unwrap_or_default()
    }

    /// Starts delivering what the adapter carries to this process's members, once; the task
    /// lives until [`stop`](Self::stop). Called from a server's `bind` or the hand-off's `bound`,
    /// inside the runtime.
    pub(crate) fn start(self: &Arc<Self>) {
        let mut delivery = self.delivery.lock().unwrap_or_else(PoisonError::into_inner);
        if delivery.is_some() {
            return;
        }
        let hub = Arc::clone(self);
        let task = tokio::spawn(async move {
            let mut carried = hub.adapter.subscribe(hub.node);
            while let Some((target, frame)) = carried.next().await {
                hub.deliver(&target, &frame);
            }
        });
        *delivery = Some(task.abort_handle());
    }

    pub(crate) fn stop(&self) {
        if let Some(task) = self.delivery.lock().unwrap_or_else(PoisonError::into_inner).take() {
            task.abort();
        }
    }

    /// One carried broadcast, re-encoded for each addressed member's gateway and queued on its
    /// connection under that connection's overflow policy.
    fn deliver(&self, target: &Target, frame: &[u8]) {
        let carried = match envelope::read_broadcast(frame) {
            Ok(carried) => carried,
            Err(error) => {
                tracing::warn!(%error, "a carried WebSocket broadcast did not decode; it is dropped");
                return;
            }
        };
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for (id, member) in members.iter() {
            if !addressed(target, *id, member) {
                continue;
            }
            let gateway = &member.gateway;
            match envelope::event(gateway.settings().codec, &gateway.event_field, &carried) {
                Ok(message) => {
                    let _ = member.outbound.push(message);
                }
                Err(error) => tracing::warn!(%error, gateway = %gateway.path, "a WebSocket broadcast could not be encoded"),
            }
        }
    }
}

fn addressed(target: &Target, id: ConnId, member: &Member) -> bool {
    if target.except.contains(&id) {
        return false;
    }
    if let Some(gateway) = &target.gateway {
        let named = *gateway == *member.gateway.path || member.gateway.namespace.as_deref() == Some(gateway.as_str());
        if !named {
            return false;
        }
    }
    match &target.audience {
        Audience::All => true,
        Audience::Room(room) => member.rooms.contains(room),
        Audience::Client(client) => *client == id,
    }
}

/// Every gateway's connections, one binding from `WsModule`. A gateway is addressed at runtime,
/// [`gateway`](Self::gateway) or [`namespace`](Self::namespace); the methods on `Rooms` itself
/// address every gateway.
#[derive(Clone)]
pub struct Rooms {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    pub(crate) hub: Arc<Hub>,
}

impl Rooms {
    /// The connections of gateway `G`, at its path with its controller's prefix once a server
    /// has prepared it, at `G::settings().path` before that.
    pub fn gateway<G: GatewayConfig>(&self) -> RoomsIn {
        let path = self.hub.path_of(TypeName::of::<G>()).map_or_else(|| G::settings().path.into_owned(), |path| path.to_string());
        RoomsIn { adapter: Arc::clone(&self.adapter), gateway: Some(path) }
    }

    /// The connections of the gateways under `namespace`.
    pub fn namespace(&self, namespace: impl Into<String>) -> RoomsIn {
        RoomsIn { adapter: Arc::clone(&self.adapter), gateway: Some(namespace.into()) }
    }

    pub fn to_all(&self) -> Broadcast {
        broadcast(&self.adapter, None, Audience::All)
    }

    pub fn to_room(&self, room: impl Into<String>) -> Broadcast {
        broadcast(&self.adapter, None, Audience::Room(room.into()))
    }

    pub fn to_client(&self, id: ConnId) -> Broadcast {
        broadcast(&self.adapter, None, Audience::Client(id))
    }
}

fn broadcast(adapter: &Arc<dyn BroadcastAdapter>, gateway: Option<String>, audience: Audience) -> Broadcast {
    Broadcast { adapter: Arc::clone(adapter), target: Target::new(gateway, audience, Vec::new()) }
}

/// One gateway's or namespace's connections, from [`Rooms::gateway`] or [`Rooms::namespace`].
#[derive(Clone)]
pub struct RoomsIn {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    pub(crate) gateway: Option<String>,
}

impl RoomsIn {
    pub fn to_all(&self) -> Broadcast {
        broadcast(&self.adapter, self.gateway.clone(), Audience::All)
    }

    pub fn to_room(&self, room: impl Into<String>) -> Broadcast {
        broadcast(&self.adapter, self.gateway.clone(), Audience::Room(room.into()))
    }

    pub fn to_client(&self, id: ConnId) -> Broadcast {
        broadcast(&self.adapter, self.gateway.clone(), Audience::Client(id))
    }
}

/// A broadcast being addressed: who receives it, less [`except`](Self::except), sent by
/// [`emit`](Self::emit).
#[must_use = "a broadcast is sent by `emit`"]
pub struct Broadcast {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    pub(crate) target: crate::broadcast::Target,
}

impl Broadcast {
    /// Leaves `ids` out.
    pub fn except(mut self, ids: impl IntoIterator<Item = ConnId>) -> Self {
        self.target.except.extend(ids);
        self
    }

    /// Sends `{"event": event, "data": data}` without an `id` to every addressed connection,
    /// encoded by each gateway's codec, through the broadcast adapter.
    ///
    /// The adapter carries it as JSON, so a MessagePack gateway receives the data as the
    /// MessagePack form of its JSON value: a byte string arrives as an array of numbers.
    pub async fn emit<T: Serialize + ?Sized>(self, event: &str, data: &T) -> Result<(), BroadcastError> {
        let frame = envelope::broadcast(event, data).map_err(BroadcastError::new)?;
        self.adapter.publish(self.target, frame).await.map_err(BroadcastError::new)
    }
}

/// A broadcast or a send that could not go out: the adapter refused it, the payload did not
/// encode, or the connection has closed. Classified `Unavailable`, so `?` in a handler returning
/// `Result<_, CallError>` renders it as one.
#[derive(Debug)]
pub struct BroadcastError {
    pub(crate) source: BoxError,
}

impl BroadcastError {
    pub fn new(source: impl Into<BoxError>) -> Self {
        BroadcastError { source: source.into() }
    }
}

impl fmt::Display for BroadcastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "broadcast failed: {}", self.source)
    }
}

impl Error for BroadcastError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

impl Classify for BroadcastError {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Unavailable
    }
}
