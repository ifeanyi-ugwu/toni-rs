use std::sync::Arc;

use redis::aio::MultiplexedConnection;
use toni::{BroadcastError, BroadcastService, RoomId, SendError, WsMessage, WsSink};

use crate::message::{BroadcastTargetKind, RedisBroadcastPayload};

/// Cross-process WebSocket broadcaster backed by Redis Pub/Sub.
///
/// Every `send()` call publishes a message to the `toni:broadcast` Redis channel.
/// All toni processes subscribed to that channel (including the publisher) receive
/// the message and deliver it to their locally connected clients — the same topology
/// Socket.io uses with `@socket.io/redis-adapter`.
///
/// Room membership is tracked per-process via the wrapped [`BroadcastService`].
/// Cross-process room queries (e.g. `get_room_clients`) only see local clients.
#[derive(Clone)]
pub struct RedisBroadcastService {
    local: BroadcastService,
    publisher: MultiplexedConnection,
    // Aborted when all clones are dropped, shutting down the subscriber loop.
    _task: Arc<tokio::task::AbortHandle>,
}

impl RedisBroadcastService {
    pub(crate) fn new(
        local: BroadcastService,
        publisher: MultiplexedConnection,
        task: tokio::task::AbortHandle,
    ) -> Self {
        Self {
            local,
            publisher,
            _task: Arc::new(task),
        }
    }

    pub fn to_all(&self) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(self.publisher.clone(), BroadcastTargetKind::All)
    }

    pub fn to_room(&self, room: impl Into<String>) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(self.publisher.clone(), BroadcastTargetKind::Room(room.into()))
    }

    pub fn to_client(&self, client_id: impl Into<String>) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(
            self.publisher.clone(),
            BroadcastTargetKind::Client(client_id.into()),
        )
    }

    pub fn except(&self, client_id: impl Into<String>) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(
            self.publisher.clone(),
            BroadcastTargetKind::Except(client_id.into()),
        )
    }

    // -------------------------------------------------------------------------
    // Room management — local-only (in-process view)
    // -------------------------------------------------------------------------

    pub fn join_room(&self, client_id: &str, room_id: &str) -> Result<(), BroadcastError> {
        self.local.join_room(client_id, room_id)
    }

    pub fn leave_room(&self, client_id: &str, room_id: &str) -> Result<(), BroadcastError> {
        self.local.leave_room(client_id, room_id)
    }

    pub fn get_client_rooms(&self, client_id: &str) -> Vec<RoomId> {
        self.local.get_client_rooms(client_id)
    }

    pub fn get_room_clients(&self, room_id: &str) -> Vec<toni::ClientId> {
        self.local.get_room_clients(room_id)
    }

    // -------------------------------------------------------------------------
    // Connection lifecycle — delegates to local BroadcastService
    // -------------------------------------------------------------------------

    pub fn connect(
        &self,
        client_id: toni::ClientId,
        sink: Arc<dyn WsSink>,
        namespace: Option<String>,
    ) {
        self.local.connect(client_id, sink, namespace);
    }

    pub fn disconnect(&self, client_id: &str) {
        self.local.disconnect(client_id);
    }
}

// =============================================================================
// RedisBroadcastTarget
// =============================================================================

/// Fluent builder returned by `RedisBroadcastService::to_*()` methods.
pub struct RedisBroadcastTarget {
    publisher: MultiplexedConnection,
    kind: BroadcastTargetKind,
    namespace: Option<String>,
}

impl RedisBroadcastTarget {
    fn new(publisher: MultiplexedConnection, kind: BroadcastTargetKind) -> Self {
        Self {
            publisher,
            kind,
            namespace: None,
        }
    }

    pub fn in_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Publish the message to Redis. Returns `Ok(0)` — delivery count is
    /// unknowable at publish time; each subscriber process counts its own.
    pub async fn send(mut self, message: WsMessage) -> Result<usize, BroadcastError> {
        let payload = RedisBroadcastPayload {
            target: self.kind,
            namespace: self.namespace,
            message,
        };
        let json = serde_json::to_string(&payload).map_err(|e| {
            tracing::error!(error = %e, "Failed to serialize broadcast payload");
            BroadcastError::SendFailed(SendError)
        })?;
        redis::cmd("PUBLISH")
            .arg("toni:broadcast")
            .arg(&json)
            .query_async::<()>(&mut self.publisher)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to publish to Redis");
                BroadcastError::SendFailed(SendError)
            })?;
        Ok(0)
    }

    /// Convenience wrapper: publishes `{"event": "...", "data": ...}` as a text message.
    pub async fn send_event(
        self,
        event: impl Into<String>,
        data: impl Into<String>,
    ) -> Result<usize, BroadcastError> {
        let msg = WsMessage::text(
            serde_json::json!({ "event": event.into(), "data": data.into() }).to_string(),
        );
        self.send(msg).await
    }
}

// =============================================================================
// Local delivery — called by the subscriber background task
// =============================================================================

/// Deliver a received Redis message to locally connected clients.
pub(crate) async fn deliver_locally(local: &BroadcastService, payload: RedisBroadcastPayload) {
    let target = match payload.target {
        BroadcastTargetKind::All => local.to_all(),
        BroadcastTargetKind::Room(r) => local.to_room(r),
        BroadcastTargetKind::Client(id) => local.to_client(id),
        BroadcastTargetKind::Except(id) => local.except(id),
    };
    let target = match payload.namespace {
        Some(ns) => target.in_namespace(ns),
        None => target,
    };
    if let Err(e) = target.send(payload.message).await {
        tracing::debug!(error = %e, "Local WebSocket delivery failed");
    }
}
