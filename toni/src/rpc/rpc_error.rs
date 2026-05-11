//! RPC handler error type.
//!
//! `RpcError` is the error carried across the RPC dispatcher and adapter
//! boundary. Handlers may return any type implementing
//! [`toni::Error`](crate::errors::Error) from their function body — the
//! [`From<E: Error>`] blanket lifts it into [`RpcError::AppError`] at
//! the macro boundary, and [`RpcError::to_data`] renders the canonical
//! envelope.
//!
//! `RpcError` does not implement [`toni::Error`](crate::errors::Error) itself; the `From` blanket
//! requires source and target to be distinct types.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::errors::Error;
use crate::rpc::RpcData;

/// Variants the RPC dispatcher returns.
///
/// [`PatternNotFound`](Self::PatternNotFound), [`Forbidden`](Self::Forbidden),
/// and [`Internal`](Self::Internal) are emitted by the framework and reach
/// the adapter as wire-Err frames (`{"err":{"status":..., "message":...}}`).
/// [`AppError`](Self::AppError) carries a user-domain error and reaches the
/// adapter as a wire-Ok frame carrying the canonical envelope
/// (`{"response":{"status":"error","kind":..., "message":...}}`).
#[derive(Debug, Clone)]
pub enum RpcError {
    /// No registered handler matched the inbound pattern.
    PatternNotFound(String),

    /// A guard rejected the message before the handler ran.
    Forbidden(String),

    /// Generic server-side failure.
    Internal(String),

    /// Carries a user-domain error implementing [`toni::Error`](crate::errors::Error). Constructed
    /// by the [`From<E: Error>`] blanket; handlers don't build this
    /// variant by hand.
    AppError(Arc<dyn Error + Send + Sync>),
}

impl RpcError {
    /// Render as an [`RpcData`] payload using the canonical envelope:
    /// `{"status":"error","kind":"...","message":...}`. For
    /// [`AppError`](Self::AppError), reads `kind` / `message` / `details`
    /// from the wrapped error; for the framework variants, uses a fixed
    /// `kind` per variant.
    pub fn to_data(&self) -> RpcData {
        match self {
            Self::AppError(e) => render_app_error(e.as_ref()),
            Self::PatternNotFound(m) => RpcData::json(json!({
                "status": "error",
                "kind": "NotFound",
                "message": m,
            })),
            Self::Forbidden(m) => RpcData::json(json!({
                "status": "error",
                "kind": "Forbidden",
                "message": m,
            })),
            Self::Internal(m) => RpcData::json(json!({
                "status": "error",
                "kind": "Internal",
                "message": m,
            })),
        }
    }
}

/// Render an arbitrary [`toni::Error`] as the canonical RPC envelope.
/// Merges `details()` into the payload when present.
pub fn render_app_error(err: &dyn Error) -> RpcData {
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
    RpcData::json(payload)
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PatternNotFound(m) => write!(f, "Pattern not found: {m}"),
            Self::Forbidden(m) => write!(f, "Guard rejected message: {m}"),
            Self::Internal(m) => write!(f, "Internal error: {m}"),
            Self::AppError(e) => write!(f, "{}: {}", e.kind().name(), e.message()),
        }
    }
}

impl std::error::Error for RpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AppError(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

/// Lift any [`toni::Error`] into [`RpcError::AppError`]. Handlers returning
/// `Result<T, MyDomainError>` use this via `?` and via the macro's auto-
/// conversion at the dispatcher boundary.
impl<E: Error> From<E> for RpcError {
    fn from(e: E) -> Self {
        Self::AppError(Arc::new(e))
    }
}
