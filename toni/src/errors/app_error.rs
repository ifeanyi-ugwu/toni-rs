//! `AppError` — the user-facing error trait that drives per-transport
//! response rendering.
//!
//! A type implementing `AppError` declares its semantic kind once and gets
//! correct HTTP / RPC / WebSocket envelopes for free. Override the
//! per-transport rendering methods when the canonical envelope isn't what
//! you want for a specific type.
//!
//! ```ignore
//! use toni::{AppError, ErrorKind};
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
//! The handler returns `Result<T, BillingError>`; the framework calls the
//! appropriate rendering method automatically based on the active transport.

use std::borrow::Cow;

use serde_json::{Value, json};

use crate::errors::HttpError;
use crate::http_helpers::{Body, HttpResponse};
use crate::rpc::RpcData;
use crate::websocket::WsMessage;

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

/// Domain-error contract that lets the framework convert your error into a
/// transport-appropriate response.
///
/// The required methods are semantic — `kind()`, `message()`, `details()`.
/// The provided methods (`into_http_response`, `into_rpc_data`,
/// `into_ws_message`) render those semantics into transport-shaped frames.
/// Override the rendering methods on a per-type basis when the canonical
/// envelope isn't right.
///
/// The rendering methods deliberately take only `&self` — context-dependent
/// decoration (request IDs, locale, tracing) belongs on response decorators
/// (interceptors, middleware), not on the error trait.
pub trait AppError: std::error::Error + Send + Sync + 'static {
    /// Coarse classification — drives default mapping across every transport.
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

    /// Render this error as an HTTP response.
    ///
    /// Default produces:
    /// ```json
    /// {
    ///   "statusCode": 404,
    ///   "message": "User not found",
    ///   "error": "Not Found",
    ///   "details": { ... }   // omitted if details() is None
    /// }
    /// ```
    /// Override for custom envelopes; the other transports' renderings still
    /// fall back to the canonical mapping.
    fn into_http_response(&self) -> HttpResponse {
        let kind = self.kind();
        let mut body = json!({
            "statusCode": kind.http_status(),
            "message": self.message(),
            "error": kind.http_reason(),
        });
        if let Some(details) = self.details()
            && let Value::Object(map) = &mut body
        {
            map.insert("details".to_string(), details);
        }
        HttpResponse {
            status: kind.http_status(),
            body: Some(Body::json(body)),
            headers: vec![],
        }
    }

    /// Render this error as an RPC payload. Default emits
    /// `{"status":"error","kind":"NotFound","message":...,"details":...}` —
    /// `kind` uses [`ErrorKind::name`] so callers can branch on it.
    fn into_rpc_data(&self) -> RpcData {
        let mut payload = json!({
            "status": "error",
            "kind": self.kind().name(),
            "message": self.message(),
        });
        if let Some(details) = self.details()
            && let Value::Object(map) = &mut payload
        {
            map.insert("details".to_string(), details);
        }
        RpcData::json(payload)
    }

    /// Render this error as a WebSocket text frame. Default emits the same
    /// shape as [`into_rpc_data`](Self::into_rpc_data) serialized as a JSON
    /// text frame so JSON-aware clients can `JSON.parse` it directly.
    fn into_ws_message(&self) -> WsMessage {
        let mut payload = json!({
            "status": "error",
            "kind": self.kind().name(),
            "message": self.message(),
        });
        if let Some(details) = self.details()
            && let Value::Object(map) = &mut payload
        {
            map.insert("details".to_string(), details);
        }
        WsMessage::text(payload.to_string())
    }
}

impl AppError for HttpError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::BadRequest(_) => ErrorKind::BadRequest,
            Self::Unauthorized(_) => ErrorKind::Unauthorized,
            Self::Forbidden(_) => ErrorKind::Forbidden,
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::Conflict(_) => ErrorKind::Conflict,
            Self::UnprocessableEntity(_) => ErrorKind::UnprocessableEntity,
            Self::InternalServerError(_) => ErrorKind::Internal,
            Self::Custom { status, .. } => match status {
                400 => ErrorKind::BadRequest,
                401 => ErrorKind::Unauthorized,
                403 => ErrorKind::Forbidden,
                404 => ErrorKind::NotFound,
                409 => ErrorKind::Conflict,
                422 => ErrorKind::UnprocessableEntity,
                429 => ErrorKind::TooManyRequests,
                501 => ErrorKind::Unimplemented,
                503 => ErrorKind::Unavailable,
                504 => ErrorKind::Timeout,
                _ => ErrorKind::Internal,
            },
        }
    }

    fn message(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.message())
    }

    /// Preserves `HttpError`'s historical envelope shape (with its
    /// `Custom { status }` carrying the literal status, not the kind's
    /// canonical mapping).
    fn into_http_response(&self) -> HttpResponse {
        self.to_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_as_json(resp: &HttpResponse) -> Value {
        let bytes = resp
            .body
            .as_ref()
            .and_then(|b| b.try_bytes())
            .expect("buffered json body");
        serde_json::from_slice(bytes).expect("valid json")
    }

    #[derive(Debug)]
    struct FakeError {
        kind: ErrorKind,
        msg: &'static str,
        details: Option<Value>,
    }

    impl std::fmt::Display for FakeError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.msg)
        }
    }

    impl std::error::Error for FakeError {}

    impl AppError for FakeError {
        fn kind(&self) -> ErrorKind {
            self.kind
        }
        fn details(&self) -> Option<Value> {
            self.details.clone()
        }
    }

    #[test]
    fn http_envelope_carries_status_kind_and_reason() {
        let e = FakeError {
            kind: ErrorKind::NotFound,
            msg: "missing",
            details: None,
        };
        let r = e.into_http_response();
        assert_eq!(r.status, 404);
        let body = body_as_json(&r);
        assert_eq!(body["statusCode"], 404);
        assert_eq!(body["error"], "Not Found");
        assert_eq!(body["message"], "missing");
        assert!(body.get("details").is_none());
    }

    #[test]
    fn http_envelope_includes_details_when_present() {
        let e = FakeError {
            kind: ErrorKind::UnprocessableEntity,
            msg: "bad",
            details: Some(json!({"field": "email"})),
        };
        let r = e.into_http_response();
        let body = body_as_json(&r);
        assert_eq!(body["details"]["field"], "email");
    }

    #[test]
    fn rpc_payload_uses_kind_name() {
        let e = FakeError {
            kind: ErrorKind::Unavailable,
            msg: "down",
            details: None,
        };
        let RpcData::Json(payload) = e.into_rpc_data() else {
            panic!("expected json variant");
        };
        assert_eq!(payload["kind"], "Unavailable");
        assert_eq!(payload["status"], "error");
        assert_eq!(payload["message"], "down");
    }

    #[test]
    fn ws_payload_is_text_frame() {
        let e = FakeError {
            kind: ErrorKind::Forbidden,
            msg: "nope",
            details: None,
        };
        let WsMessage::Text(s) = e.into_ws_message() else {
            panic!("expected text frame");
        };
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["kind"], "Forbidden");
    }

    #[test]
    fn http_error_uses_to_response_for_envelope_compat() {
        let err = HttpError::not_found("nope");
        let r = err.into_http_response();
        assert_eq!(r.status, 404);
        let body = body_as_json(&r);
        // HttpError preserves its historical envelope — `error` is "Not Found"
        // (matches both `error_type()` and `ErrorKind::NotFound.http_reason()`).
        assert_eq!(body["error"], "Not Found");
    }

    #[test]
    fn http_error_kind_matches_status() {
        assert_eq!(HttpError::not_found("x").kind(), ErrorKind::NotFound);
        assert_eq!(HttpError::conflict("x").kind(), ErrorKind::Conflict);
        assert_eq!(HttpError::custom(429, "x").kind(), ErrorKind::TooManyRequests);
        assert_eq!(HttpError::custom(418, "x").kind(), ErrorKind::Internal);
    }

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
