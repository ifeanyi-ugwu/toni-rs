//! Typed events the HTTP dispatcher emits when *the framework itself* is the
//! source of an error — guard rejections, middleware failures, and similar
//! cases that don't originate from a user handler.
//!
//! These types replace the old pattern of synthesising a generic `HttpError`
//! from the response status: instead of "the response is 4xx, reconstruct an
//! error proxy from it," the dispatcher names what actually happened. Chain
//! handlers and observers can downcast to the concrete event and react to
//! the underlying cause.
//!
//! `HttpError` survives as a user-convenience type for trivial handler
//! returns (`HttpError::not_found("user 42")`) — it is no longer load-bearing
//! for framework-emitted events.

use std::borrow::Cow;
use std::fmt;

use crate::errors::{AppError, ErrorKind};

/// Emitted when an HTTP guard returns `false` (or aborts). The chain runs on
/// this event before the framework's default 403 envelope is rendered.
#[derive(Debug, Clone)]
pub struct GuardRejection {
    /// Zero-based position of the rejecting guard in the resolved chain.
    pub guard_index: usize,
    /// Free-form reason. `None` when the guard rejected without a message.
    pub reason: Option<String>,
}

impl GuardRejection {
    pub fn new(guard_index: usize) -> Self {
        Self {
            guard_index,
            reason: None,
        }
    }

    pub fn with_reason(guard_index: usize, reason: impl Into<String>) -> Self {
        Self {
            guard_index,
            reason: Some(reason.into()),
        }
    }
}

impl fmt::Display for GuardRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            Some(r) => write!(f, "guard {} rejected request: {r}", self.guard_index),
            None => write!(f, "guard {} rejected request", self.guard_index),
        }
    }
}

impl std::error::Error for GuardRejection {}

impl AppError for GuardRejection {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Forbidden
    }

    fn message(&self) -> Cow<'_, str> {
        match &self.reason {
            Some(r) => Cow::Borrowed(r.as_str()),
            None => Cow::Borrowed("Forbidden"),
        }
    }
}

/// Emitted when a middleware in the chain returned `Err` before the request
/// reached the route handler. Carries the source error's message for the
/// chain to inspect; the source itself is preserved on `source` so handlers
/// can still downcast through it when they own the underlying type.
#[derive(Debug)]
pub struct MiddlewareFailure {
    pub message: String,
}

impl MiddlewareFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MiddlewareFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "middleware failed: {}", self.message)
    }
}

impl std::error::Error for MiddlewareFailure {}

impl AppError for MiddlewareFailure {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }

    fn message(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.message.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_rejection_renders_403_via_app_error() {
        let event = GuardRejection::with_reason(0, "missing token");
        let resp = event.into_http_response();
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn middleware_failure_renders_500_via_app_error() {
        let event = MiddlewareFailure::new("DB pool exhausted");
        let resp = event.into_http_response();
        assert_eq!(resp.status, 500);
    }
}
