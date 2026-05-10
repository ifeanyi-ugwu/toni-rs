//! WebSocket handler error type — owns rendering, wraps domain `AppError`s.
//!
//! `WsError` is the canonical error type the WebSocket dispatcher flows
//! through its pipeline. Handlers don't have to return it directly: any
//! type implementing [`AppError`](crate::errors::AppError) gets a free
//! conversion via [`From<E: AppError> for WsError`], so writing
//! `WsHandlerResult` with a domain error type from a `#[subscribe_message]`
//! handler is fine — the framework lifts the domain error into
//! [`WsError::AppError`] at the dispatcher boundary and renders the
//! canonical text-frame envelope.
//!
//! `WsError` itself does **not** implement `AppError`. Same reason as
//! [`HttpError`](crate::errors::HttpError) and
//! [`RpcError`](crate::errors::RpcError) — the `From<E: AppError>` blanket
//! must not collide with std's reflexive `From<T> for T`.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::websocket::WsMessage;

/// WebSocket error kinds plus the override slots: a fully-formed
/// [`WsMessage`] (the override path), and a wrapper around any
/// [`AppError`] (the canonical-envelope path).
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

    /// Wraps a domain error implementing [`AppError`]. Renders through
    /// the canonical text-frame envelope. Constructed automatically via
    /// the [`From<E: AppError>`] blanket — handlers normally don't build
    /// this variant directly.
    AppError(Arc<dyn AppError + Send + Sync>),
}

impl WsError {
    /// Render this error as a [`WsMessage`].
    ///
    /// Named variants produce a JSON text frame:
    /// `{"status":"error","kind":"...","message":...}`.
    /// [`Self::Response`] returns its wrapped frame. [`Self::AppError`]
    /// renders the wrapped error's `kind` / `message` / `details` through
    /// the canonical shape.
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

/// Canonical WebSocket text-frame rendering for an arbitrary [`AppError`].
/// Used by [`WsError::AppError`] and by framework internals that need to
/// render an `AppError` to WS without going through the wrapper variant.
pub fn render_app_error(err: &dyn AppError) -> WsMessage {
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

/// Lift any [`AppError`] into [`WsError::AppError`] so handlers returning
/// `Result<T, MyDomainError>` work via `?` and the macro's auto-conversion
/// at the dispatcher boundary.
impl<E: AppError> From<E> for WsError {
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
