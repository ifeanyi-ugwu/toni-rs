//! `AppError` — the semantic error contract.
//!
//! A type implementing `AppError` declares its semantic kind once via
//! [`kind`](AppError::kind). Per-transport rendering belongs to the
//! transport's handler error type — [`HttpError`](crate::errors::HttpError),
//! [`RpcError`](crate::errors::RpcError),
//! [`WsError`](crate::errors::WsError) — each of which carries its own wire
//! shape and provides a `From<E: AppError>` blanket so domain errors flow
//! into the right transport via `?` at the handler boundary.
//!
//! ```ignore
//! use toni::errors::{AppError, ErrorKind};
//!
//! #[derive(Debug, thiserror::Error)]
//! enum BillingError {
//!     #[error("invoice {0} not found")]
//!     InvoiceNotFound(String),
//!     #[error("card declined")]
//!     CardDeclined,
//! }
//!
//! impl AppError for BillingError {
//!     fn kind(&self) -> ErrorKind {
//!         match self {
//!             Self::InvoiceNotFound(_) => ErrorKind::NotFound,
//!             Self::CardDeclined       => ErrorKind::UnprocessableEntity,
//!         }
//!     }
//! }
//! ```
//!
//! The handler returns `Result<T, BillingError>`; the macro converts the
//! `Err` arm to the active transport's error type via `From<BillingError>
//! for HttpError` (or `RpcError` / `WsError`), and the dispatcher renders
//! the canonical envelope from `kind` / `message` / `details`.

use std::borrow::Cow;

use serde_json::Value;

/// Coarse classification of error semantics, transport-independent.
///
/// Each transport's default rendering reads from this taxonomy:
/// HTTP maps to status codes, RPC and WebSocket to a stable status string.
/// The kind layer means a single `AppError` impl produces the right shape
/// on every transport without per-transport conversion code.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// 400 — request was malformed or carried invalid data.
    BadRequest,
    /// 401 — authentication missing or invalid.
    Unauthorized,
    /// 403 — authenticated but forbidden from this resource.
    Forbidden,
    /// 404 — requested resource does not exist.
    NotFound,
    /// 409 — conflict with current state (duplicate, version mismatch).
    Conflict,
    /// 422 — well-formed but semantically invalid.
    UnprocessableEntity,
    /// 429 — caller exceeded a rate limit.
    TooManyRequests,
    /// 408 / 504 — the operation did not complete in time.
    Timeout,
    /// 503 — a backend dependency is unavailable.
    Unavailable,
    /// 501 — the operation is not implemented for this resource.
    Unimplemented,
    /// 500 — generic server-side failure.
    Internal,
}

impl ErrorKind {
    /// Stable identifier suitable for serialising into wire payloads. The
    /// string is the variant name (e.g. `"NotFound"`) so it is forward-stable
    /// and grep-able across language ecosystems.
    pub fn name(self) -> &'static str {
        match self {
            Self::BadRequest => "BadRequest",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "NotFound",
            Self::Conflict => "Conflict",
            Self::UnprocessableEntity => "UnprocessableEntity",
            Self::TooManyRequests => "TooManyRequests",
            Self::Timeout => "Timeout",
            Self::Unavailable => "Unavailable",
            Self::Unimplemented => "Unimplemented",
            Self::Internal => "Internal",
        }
    }

    /// HTTP status code this kind maps to.
    pub fn http_status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::UnprocessableEntity => 422,
            Self::TooManyRequests => 429,
            Self::Timeout => 504,
            Self::Unavailable => 503,
            Self::Unimplemented => 501,
            Self::Internal => 500,
        }
    }

    /// HTTP reason phrase for response envelopes.
    pub fn http_reason(self) -> &'static str {
        match self {
            Self::BadRequest => "Bad Request",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not Found",
            Self::Conflict => "Conflict",
            Self::UnprocessableEntity => "Unprocessable Entity",
            Self::TooManyRequests => "Too Many Requests",
            Self::Timeout => "Gateway Timeout",
            Self::Unavailable => "Service Unavailable",
            Self::Unimplemented => "Not Implemented",
            Self::Internal => "Internal Server Error",
        }
    }
}

/// Domain-error contract — pure semantic info.
///
/// Implementing `AppError` makes the type renderable on every transport
/// via the framework's per-transport `From<E: AppError>` blankets — domain
/// errors `?`-flow into [`HttpError`](crate::errors::HttpError),
/// [`RpcError`](crate::errors::RpcError), and
/// [`WsError`](crate::errors::WsError) automatically. The transport's
/// handler error type owns the rendering; this trait owns the semantic
/// vocabulary.
pub trait AppError: std::error::Error + Send + Sync + 'static {
    /// Coarse classification — drives the canonical envelope on every transport.
    fn kind(&self) -> ErrorKind;

    /// Human-readable explanation for the client. Default uses `Display`,
    /// which is usually what `thiserror`-derived enums produce.
    fn message(&self) -> Cow<'_, str> {
        Cow::Owned(self.to_string())
    }

    /// Structured payload merged into the response envelope under
    /// `details`. Use for field-level validation results, retry hints,
    /// trace ids, or anything the client needs beyond the message.
    fn details(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_http_status_round_trip() {
        assert_eq!(ErrorKind::NotFound.http_status(), 404);
        assert_eq!(ErrorKind::Timeout.http_status(), 504);
        assert_eq!(ErrorKind::Unavailable.http_status(), 503);
    }

    #[test]
    fn kind_name_is_stable() {
        // Wire payloads serialize this — guard against accidental rename.
        assert_eq!(ErrorKind::NotFound.name(), "NotFound");
        assert_eq!(ErrorKind::UnprocessableEntity.name(), "UnprocessableEntity");
    }
}
