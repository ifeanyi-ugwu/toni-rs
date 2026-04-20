use std::sync::Arc;

use redis::aio::MultiplexedConnection;
use toni::{BroadcastError, BroadcastService, ClientId, RoomId, SendError, WsMessage, WsSink};

use crate::message::{BroadcastTargetKind, RedisBroadcastPayload};

/// Cross-process WebSocket broadcaster backed by Redis Pub/Sub.
///
/// Every `send()` call publishes a message to a Redis channel. `to_all`,
/// `to_room`, and `except` publish to the shared `toni:broadcast` channel;
/// `to_client` looks up which process owns the target client and publishes
/// directly to that process's private channel, avoiding global fan-out.
///
/// Room membership is stored in Redis sets, so `join_room`/`leave_room` and
/// the corresponding queries reflect the global state across all processes.
#[derive(Clone)]
pub struct RedisBroadcastService {
    local: BroadcastService,
    publisher: MultiplexedConnection,
    /// Unique identifier for this process instance. Stored in Redis when a
    /// client connects so other processes can publish directly to this channel.
    process_id: String,
    // Aborted when all clones are dropped, shutting down the subscriber loop.
    _task: Arc<tokio::task::AbortHandle>,
}

impl RedisBroadcastService {
    pub(crate) fn new(
        local: BroadcastService,
        publisher: MultiplexedConnection,
        process_id: String,
        task: tokio::task::AbortHandle,
    ) -> Self {
        Self {
            local,
            publisher,
            process_id,
            _task: Arc::new(task),
        }
    }

    pub fn to_all(&self) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(self.publisher.clone(), BroadcastTargetKind::All)
    }

    pub fn to_room(&self, room: impl Into<String>) -> RedisBroadcastTarget {
        RedisBroadcastTarget::new(self.publisher.clone(), BroadcastTargetKind::Room(room.into()))
    }

    /// Publishes directly to the channel of the process that owns `client_id`,
    /// avoiding global fan-out. Falls back to the global channel if the client
    /// is not found in Redis (e.g. already disconnected).
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
    // Room management — global view via Redis sets
    //
    // Redis keys:
    //   toni:rooms:{room_id}          → SET of client_ids in the room
    //   toni:client:{client_id}:rooms → SET of room_ids the client belongs to
    // -------------------------------------------------------------------------

    pub async fn join_room(&self, client_id: &str, room_id: &str) -> Result<(), BroadcastError> {
        self.local.join_room(client_id, room_id)?;
        let mut conn = self.publisher.clone();
        if let Err(e) = redis::pipe()
            .sadd(format!("toni:rooms:{room_id}"), client_id)
            .ignore()
            .sadd(format!("toni:client:{client_id}:rooms"), room_id)
            .ignore()
            .query_async::<()>(&mut conn)
            .await
        {
            tracing::warn!(error = %e, client_id, room_id, "Failed to sync join_room to Redis");
        }
        Ok(())
    }

    pub async fn leave_room(&self, client_id: &str, room_id: &str) -> Result<(), BroadcastError> {
        self.local.leave_room(client_id, room_id)?;
        let mut conn = self.publisher.clone();
        if let Err(e) = redis::pipe()
            .srem(format!("toni:rooms:{room_id}"), client_id)
            .ignore()
            .srem(format!("toni:client:{client_id}:rooms"), room_id)
            .ignore()
            .query_async::<()>(&mut conn)
            .await
        {
            tracing::warn!(error = %e, client_id, room_id, "Failed to sync leave_room to Redis");
        }
        Ok(())
    }

    /// Returns all clients in a room across all processes.
    pub async fn get_room_clients(&self, room_id: &str) -> Vec<ClientId> {
        let mut conn = self.publisher.clone();
        match redis::cmd("SMEMBERS")
            .arg(format!("toni:rooms:{room_id}"))
            .query_async::<Vec<String>>(&mut conn)
            .await
        {
            Ok(members) => members,
            Err(e) => {
                tracing::warn!(error = %e, room_id, "Failed to query room members from Redis, falling back to local");
                self.local.get_room_clients(room_id)
            }
        }
    }

    /// Returns all rooms a client belongs to, as recorded in Redis.
    pub async fn get_client_rooms(&self, client_id: &str) -> Vec<RoomId> {
        let mut conn = self.publisher.clone();
        match redis::cmd("SMEMBERS")
            .arg(format!("toni:client:{client_id}:rooms"))
            .query_async::<Vec<String>>(&mut conn)
            .await
        {
            Ok(rooms) => rooms,
            Err(e) => {
                tracing::warn!(error = %e, client_id, "Failed to query client rooms from Redis, falling back to local");
                self.local.get_client_rooms(client_id)
            }
        }
    }

    // -------------------------------------------------------------------------
    // Connection lifecycle
    // -------------------------------------------------------------------------

    pub async fn connect(&self, client_id: ClientId, sink: Arc<dyn WsSink>, namespace: Option<String>) {
        self.local.connect(client_id.clone(), sink, namespace);
        let mut conn = self.publisher.clone();
        if let Err(e) = redis::pipe()
            // Auto-room that ConnectionManager::register() creates locally.
            .sadd(format!("toni:rooms:{client_id}"), &client_id)
            .ignore()
            .sadd(format!("toni:client:{client_id}:rooms"), &client_id)
            .ignore()
            // Process affinity: lets other processes publish to_client directly
            // to our private channel instead of the global one.
            .set(format!("toni:client:{client_id}:process"), &self.process_id)
            .ignore()
            .query_async::<()>(&mut conn)
            .await
        {
            tracing::warn!(error = %e, %client_id, "Failed to sync client connect to Redis");
        }
    }

    pub async fn disconnect(&self, client_id: &str) {
        let rooms = self.local.get_client_rooms(client_id);
        self.local.disconnect(client_id);

        let mut conn = self.publisher.clone();
        let mut pipe = redis::pipe();
        for room in &rooms {
            pipe.srem(format!("toni:rooms:{room}"), client_id).ignore();
        }
        pipe.del(format!("toni:client:{client_id}:rooms"))
            .ignore()
            .del(format!("toni:client:{client_id}:process"))
            .ignore();
        if let Err(e) = pipe.query_async::<()>(&mut conn).await {
            tracing::warn!(error = %e, client_id, "Failed to sync client disconnect to Redis");
        }
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
        // For to_client, look up the target client's process and publish to its
        // private channel. All other target types use the global channel.
        let channel = if let BroadcastTargetKind::Client(ref client_id) = self.kind {
            match redis::cmd("GET")
                .arg(format!("toni:client:{client_id}:process"))
                .query_async::<Option<String>>(&mut self.publisher)
                .await
            {
                Ok(Some(process_id)) => format!("toni:broadcast:{process_id}"),
                _ => "toni:broadcast".to_string(),
            }
        } else {
            "toni:broadcast".to_string()
        };

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
            .arg(&channel)
            .arg(&json)
            .query_async::<()>(&mut self.publisher)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to publish to Redis channel '{channel}'");
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
