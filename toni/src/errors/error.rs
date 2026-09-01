//! `toni::Error` — the framework's error contract.
//!
//! A type implementing `Error` declares its semantic [`kind`](Error::kind);
//! the transport's handler error type ([`HttpError`](crate::errors::HttpError),
//! [`RpcError`](crate::rpc::RpcError),
//! [`WsError`](crate::websocket::WsError)) carries the wire shape and provides
//! a `From<E: Error>` blanket so a `toni::Error` returned by a handler flows
//! into the right transport via `?`.
//!
//! ```ignore
//! use toni::{Error, ErrorKind};
//!
//! #[derive(Debug, thiserror::Error)]
//! enum BillingError {
//!     #[error("invoice {0} not found")]
//!     InvoiceNotFound(String),
//!     #[error("card declined")]
//!     CardDeclined,
//! }
//!
//! impl Error for BillingError {
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
//! `Err` arm to the active transport's error type, and the dispatcher
//! renders the canonical envelope from `kind` / `message` / `details`.

use std::borrow::Cow;

use serde_json::Value;

/// Coarse classification of error semantics, transport-independent.
///
/// Each transport's rendering layer maps a kind to its own wire form
/// (HTTP status codes via [`http_status`](crate::errors::http_status),
/// RPC/WS status strings via [`name`](Self::name)). The kind layer means
/// a single [`Error`] impl produces the right shape on every transport
/// without per-transport conversion code on the error type itself.
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
    /// Stable identifier for wire payloads — the variant name (`"NotFound"`).
    /// Clients in any language parse this string, so it is part of toni's
    /// public wire API: renaming any of these is a breaking change.
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
}

/// The framework's error contract — `std::error::Error` plus the metadata
/// the toni pipeline needs (`kind` for chain dispatch and rendering;
/// `message`; `details`).
///
/// Implementing `Error` makes the type renderable on every transport via
/// the per-transport `From<E: Error>` blankets — it `?`-flows into
/// [`HttpError`](crate::errors::HttpError),
/// [`RpcError`](crate::rpc::RpcError), and
/// [`WsError`](crate::websocket::WsError) automatically. The transport's
/// handler error type owns the rendering; this trait owns the semantic
/// vocabulary.
pub trait Error: std::error::Error + Send + Sync + 'static {
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
