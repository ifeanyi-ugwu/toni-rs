//! WebSocket handler error type.
//!
//! `WsError` is the error carried across the WebSocket dispatcher and
//! adapter boundary. Handlers may return any type implementing
//! [`toni::Error`](crate::errors::Error) from their function body — the
//! [`From<E: Error>`] blanket lifts it into [`WsError::AppError`] at the
//! macro boundary, and [`WsError::to_message`] renders the canonical
//! text-frame envelope.
//!
//! `WsError` does not implement [`toni::Error`](crate::errors::Error); the `From` blanket requires
//! source and target to be distinct types.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::errors::Error;
use crate::websocket::WsMessage;

/// WebSocket error variants — framework-emitted kinds plus a wrapper for
/// user-domain [`toni::Error`](crate::errors::Error) values.
#[derive(Debug, Clone)]
pub enum WsError {
    /// The connection closed before the handler could finish.
    ConnectionClosed(String),

    /// Inbound frame couldn't be parsed into a known event shape.
    InvalidMessage(String),

    /// A guard rejected the connection or message.
    AuthFailed(String),

    /// The inbound event name has no registered handler.
    EventNotFound(String),

    /// Generic server-side failure.
    Internal(String),

    /// Forwarded from the broadcast subsystem.
    BroadcastError(String),

    /// Carries a user-domain error implementing
    /// [`toni::Error`](crate::errors::Error). Constructed by the
    /// [`From<E: Error>`] blanket; handlers don't build this variant by
    /// hand.
    AppError(Arc<dyn Error + Send + Sync>),
}

impl WsError {
    /// Render as a [`WsMessage`] using the canonical text-frame envelope:
    /// `{"status":"error","kind":"...","message":...}`. For
    /// [`AppError`](Self::AppError), reads `kind` / `message` / `details`
    /// from the wrapped error; the named variants use a fixed `kind`
    /// per variant.
    pub fn to_message(&self) -> WsMessage {
        match self {
            Self::AppError(e) => render_app_error(e.as_ref()),
            other => {
                let (kind_name, message) = match other {
                    Self::ConnectionClosed(m) => ("Unavailable", m.as_str()),
                    Self::InvalidMessage(m) => ("BadRequest", m.as_str()),
                    Self::AuthFailed(m) => ("Unauthorized", m.as_str()),
                    Self::EventNotFound(m) => ("NotFound", m.as_str()),
                    Self::Internal(m) | Self::BroadcastError(m) => ("Internal", m.as_str()),
                    Self::AppError(_) => unreachable!(),
                };
                let payload = json!({
                    "status": "error",
                    "kind": kind_name,
                    "message": message,
                });
                WsMessage::text(payload.to_string())
            }
        }
    }
}

/// Render an arbitrary [`toni::Error`] as the canonical WebSocket text-frame
/// envelope. Merges `details()` into the payload when present.
pub fn render_app_error(err: &dyn Error) -> WsMessage {
    let mut payload = json!({
        "status": "error",
        "kind": err.kind().name(),
        "message": err.message(),
    });
    if let Some(details) = err.details()
        && let Value::Object(map) = &mut payload
    {
        map.insert("details".to_string(), details);
    }
    WsMessage::text(payload.to_string())
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionClosed(m) => write!(f, "Connection closed: {m}"),
            Self::InvalidMessage(m) => write!(f, "Invalid message format: {m}"),
            Self::AuthFailed(m) => write!(f, "Authentication failed: {m}"),
            Self::EventNotFound(m) => write!(f, "Event not found: {m}"),
            Self::Internal(m) => write!(f, "Internal error: {m}"),
            Self::BroadcastError(m) => write!(f, "Broadcast error: {m}"),
            Self::AppError(e) => write!(f, "{}: {}", e.kind().name(), e.message()),
        }
    }
}

impl std::error::Error for WsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AppError(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<crate::websocket::BroadcastError> for WsError {
    fn from(err: crate::websocket::BroadcastError) -> Self {
        WsError::BroadcastError(err.to_string())
    }
}

/// Lift any [`toni::Error`] into [`WsError::AppError`]. Handlers returning
/// `Result<T, MyDomainError>` use this via `?` and via the macro's auto-
/// conversion at the dispatcher boundary.
impl<E: Error> From<E> for WsError {
    fn from(e: E) -> Self {
        Self::AppError(Arc::new(e))
    }
}

/// Reason for client disconnection
#[derive(Debug, Clone)]
pub enum DisconnectReason {
    ClientDisconnect,
    ServerShutdown,
    Timeout,
    Error(String),
}

impl DisconnectReason {
    pub fn error(msg: impl Into<String>) -> Self {
        Self::Error(msg.into())
    }
}
