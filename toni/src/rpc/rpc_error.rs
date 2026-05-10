//! RPC handler error type — owns rendering, wraps domain `AppError`s.
//!
//! `RpcError` is the canonical error type the RPC dispatcher flows through
//! its pipeline. Handlers don't have to return it directly: any type
//! implementing [`AppError`](crate::errors::AppError) gets a free
//! conversion via [`From<E: AppError> for RpcError`], so writing
//! `Result<RpcData, MyDomainError>` from an RPC handler is fine — the
//! framework lifts the domain error into [`RpcError::AppError`] at the
//! dispatcher boundary and renders the canonical `RpcData` envelope.
//!
//! `RpcError` itself does **not** implement `AppError`. Same reason as
//! [`HttpError`](crate::errors::HttpError): keeps the `From<E: AppError>`
//! blanket from conflicting with std's reflexive `From<T> for T`.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::rpc::RpcData;

/// RPC error kinds plus the override slots: a fully-formed [`RpcData`]
/// payload, and a wrapper around any [`AppError`].
#[derive(Debug, Clone)]
pub enum RpcError {
    /// No registered handler matched the inbound pattern.
    PatternNotFound(String),

    /// A guard rejected the message before the handler ran.
    Forbidden(String),

    /// Generic server-side failure.
    Internal(String),

    /// Wraps a domain error implementing [`AppError`]. Renders through
    /// the canonical envelope derived from the error's
    /// [`kind`](crate::errors::AppError::kind). Constructed automatically
    /// via the [`From<E: AppError>`] blanket — handlers normally don't
    /// build this variant directly.
    AppError(Arc<dyn AppError + Send + Sync>),
}

impl RpcError {
    /// Render this error as an [`RpcData`] payload.
    ///
    /// Named variants produce `{"status":"error","kind":"...","message":...}`.
    /// [`Self::Response`] returns its wrapped payload. [`Self::AppError`]
    /// renders the wrapped error's `kind` / `message` / `details` through
    /// the canonical shape.
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

/// Canonical RPC envelope rendering for an arbitrary [`AppError`]. Used by
/// [`RpcError::AppError`] and by framework internals that need to render an
/// `AppError` to RPC without going through the wrapper variant.
pub fn render_app_error(err: &dyn AppError) -> RpcData {
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

/// Lift any [`AppError`] into [`RpcError::AppError`] so handlers returning
/// `Result<T, MyDomainError>` work via `?` and the macro's auto-conversion
/// at the dispatcher boundary.
impl<E: AppError> From<E> for RpcError {
    fn from(e: E) -> Self {
        Self::AppError(Arc::new(e))
    }
}
