//! Rooms: addressing a gateway's connections from any code that holds `Dep<Rooms>` (transports
//! DESIGN §4.3).
//!
//! ```ignore
//! rooms.to_all().emit("notice", &n).await?;
//! rooms.gateway::<ChatGateway>().to_room("lobby").except([me]).emit("chat.message", &m).await?;
//! rooms.to_client(id).emit("dm", &dm).await?;
//! ```

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use ulo::BoxError;
use ulo_transport::{Classify, ErrorKind};

use crate::broadcast::BroadcastAdapter;
use crate::connection::ConnId;
use crate::gateway::GatewayConfig;

/// Every gateway's connections, one binding from `WsModule`. A gateway is addressed at runtime,
/// [`gateway`](Self::gateway) or [`namespace`](Self::namespace); the methods on `Rooms` itself
/// address every gateway.
#[derive(Clone)]
pub struct Rooms {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
}

impl Rooms {
    /// The connections of gateway `G`.
    pub fn gateway<G: GatewayConfig>(&self) -> RoomsIn {
        todo!()
    }

    /// The connections of the gateways under `namespace`.
    pub fn namespace(&self, namespace: impl Into<String>) -> RoomsIn {
        let _ = namespace;
        todo!()
    }

    pub fn to_all(&self) -> Broadcast {
        todo!()
    }

    pub fn to_room(&self, room: impl Into<String>) -> Broadcast {
        let _ = room;
        todo!()
    }

    pub fn to_client(&self, id: ConnId) -> Broadcast {
        let _ = id;
        todo!()
    }
}

/// One gateway's or namespace's connections, from [`Rooms::gateway`] or [`Rooms::namespace`].
#[derive(Clone)]
pub struct RoomsIn {
    pub(crate) adapter: Arc<dyn BroadcastAdapter>,
    pub(crate) gateway: Option<String>,
}

impl RoomsIn {
    pub fn to_all(&self) -> Broadcast {
        todo!()
    }

    pub fn to_room(&self, room: impl Into<String>) -> Broadcast {
        let _ = room;
        todo!()
    }

    pub fn to_client(&self, id: ConnId) -> Broadcast {
        let _ = id;
        todo!()
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
    pub fn except(self, ids: impl IntoIterator<Item = ConnId>) -> Self {
        let _ = ids;
        todo!()
    }

    /// Sends `{"event": event, "data": data}` without an `id` to every addressed connection,
    /// encoded by each gateway's codec, through the broadcast adapter.
    pub async fn emit<T: Serialize + ?Sized>(self, event: &str, data: &T) -> Result<(), BroadcastError> {
        let _ = (event, data);
        todo!()
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
