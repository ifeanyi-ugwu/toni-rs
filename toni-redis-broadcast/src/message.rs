use serde::{Deserialize, Serialize};
use toni::WsMessage;

/// Envelope published to the `toni:broadcast` Redis channel.
///
/// Every broadcast call serializes to this format, and every subscriber
/// deserializes from it to deliver messages to locally connected clients.
#[derive(Serialize, Deserialize)]
pub(crate) struct RedisBroadcastPayload {
    pub target: BroadcastTargetKind,
    pub namespace: Option<String>,
    pub message: WsMessage,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) enum BroadcastTargetKind {
    All,
    Room(String),
    Client(String),
    Except(String),
}
