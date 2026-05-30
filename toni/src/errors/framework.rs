//! Typed events the framework emits when it — not a user handler — is the
//! source of an error: guard rejections, middleware failures, panic
//! recovery, client give-up. They implement [`Error`](crate::errors::Error)
//! and flow through the same observer + chain pipeline as user errors;
//! chain handlers and observers can downcast to the concrete event to react
//! to the underlying cause.

use std::borrow::Cow;
use std::fmt;

use crate::errors::{Error, ErrorKind};

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

impl Error for GuardRejection {
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

/// Emitted when a middleware returned `Err` before the request reached the
/// route handler. Carries the failing error's message; `kind()` is
/// `Internal`.
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

impl Error for MiddlewareFailure {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }

    fn message(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.message.as_str())
    }
}

/// Where in the request pipeline a panic was caught. Carried on
/// [`PanicRecovered`] so observers and chain handlers can branch on the
/// site without parsing the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PipelineSegment {
    /// Inside the user handler's own body.
    HandlerBody,
    /// Inside the active transport rendering an error to its wire shape.
    ResponseRendering,
    /// Inside an interceptor / middleware chain step.
    Middleware,
    /// Inside a pipe's `process(&mut C)`.
    Pipe,
    /// Inside a guard's `can_activate`.
    Guard,
    /// Inside a registered chain handler.
    ErrorHandler,
    /// Anywhere else inside the framework's dispatch.
    Other,
}

impl PipelineSegment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HandlerBody => "handler",
            Self::ResponseRendering => "response_rendering",
            Self::Middleware => "middleware",
            Self::Pipe => "pipe",
            Self::Guard => "guard",
            Self::ErrorHandler => "error_handler",
            Self::Other => "other",
        }
    }
}

/// Emitted when the framework caught an unwind from user or framework
/// code. Carries the segment where the panic happened so debugging
/// context isn't lost — `PanicRecovered::during(HandlerBody)` and
/// `PanicRecovered::during(Middleware)` are different stories even if
/// they end up rendering the same 500.
#[derive(Debug)]
pub struct PanicRecovered {
    pub during: PipelineSegment,
    /// Best-effort string extracted from the panic payload. The std
    /// convention is `&'static str` or `String`; anything else degrades
    /// to a placeholder.
    pub message: String,
}

impl PanicRecovered {
    pub fn during(segment: PipelineSegment) -> Self {
        Self {
            during: segment,
            message: String::new(),
        }
    }

    pub fn with_message(segment: PipelineSegment, message: impl Into<String>) -> Self {
        Self {
            during: segment,
            message: message.into(),
        }
    }

    /// Build from the payload `std::panic::catch_unwind` returns. Tries to
    /// extract a `String` or `&'static str`; otherwise records a generic
    /// `"<panic payload was not a string>"` placeholder.
    pub fn from_panic_payload(
        segment: PipelineSegment,
        payload: Box<dyn std::any::Any + Send>,
    ) -> Self {
        let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<panic payload was not a string>".to_string()
        };
        Self {
            during: segment,
            message,
        }
    }
}

impl fmt::Display for PanicRecovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "panic recovered in {}", self.during.as_str())
        } else {
            write!(
                f,
                "panic recovered in {}: {}",
                self.during.as_str(),
                self.message,
            )
        }
    }
}

impl std::error::Error for PanicRecovered {}

impl Error for PanicRecovered {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Emitted when the framework detected the client gave up — disconnect
/// before the handler finished, or a deadline / cancellation token
/// firing. Observer-only by convention: there's no response to override
/// because the client isn't listening anymore.
#[derive(Debug, Clone)]
pub struct Cancelled {
    pub reason: String,
}

impl Cancelled {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request cancelled: {}", self.reason)
    }
}

impl std::error::Error for Cancelled {}

impl Error for Cancelled {
    fn kind(&self) -> ErrorKind {
        // Cancelled is observer-only by convention; if it ever does flow
        // through the chain and nothing claims, this is the most honest
        // surface code: 499 isn't an option in our taxonomy and wrapping
        // it as a 500 would lie about server failure.
        ErrorKind::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::http_error::render_error;

    #[test]
    fn guard_rejection_renders_403() {
        let event = GuardRejection::with_reason(0, "missing token");
        let resp = render_error(&event);
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn middleware_failure_renders_500() {
        let event = MiddlewareFailure::new("DB pool exhausted");
        let resp = render_error(&event);
        assert_eq!(resp.status, 500);
    }
}
