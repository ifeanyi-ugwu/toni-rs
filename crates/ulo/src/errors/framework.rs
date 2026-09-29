//! Typed events the framework emits when it — not a user handler — is the
//! source of an error: guard rejections, middleware failures, panic
//! recovery. They implement [`crate::errors::Error`] and flow through the
//! same error-handler chain as user errors, so a `#[catch]` handler can
//! downcast to the concrete event and react to the underlying cause.

use std::borrow::Cow;
use std::fmt;

use crate::errors::{Error, ErrorKind};

/// Emitted when a guard returns `false`, on all four transports. The chain runs on this event
/// before the transport's own rejection is rendered — 403 on HTTP; on RPC, the canonical
/// `Forbidden` envelope on the `response` lane; `PermissionDenied` on gRPC; the canonical envelope
/// on WebSocket.
///
/// A WebSocket *connect* guard is the exception: its `false` refuses the upgrade. A refused
/// connection has no answer to shape, so no event is raised.
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

/// Emitted when nothing was registered to answer a call — an RPC pattern no
/// controller claims, a WebSocket event no handler subscribes to.
///
/// The condition an operator most wants to see, since it usually means a caller
/// and a server disagree about what exists. Carries the target as the caller
/// named it; `kind()` is `NotFound`.
#[derive(Debug)]
pub struct Unrouted {
    pub target: String,
}

impl Unrouted {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
        }
    }
}

impl fmt::Display for Unrouted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "nothing handles {}", self.target)
    }
}

impl std::error::Error for Unrouted {}

impl Error for Unrouted {
    fn kind(&self) -> ErrorKind {
        ErrorKind::NotFound
    }

    fn message(&self) -> Cow<'_, str> {
        Cow::Owned(format!("nothing handles {}", self.target))
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

/// Emitted when a dispatch target built per call returns `Err` from its `#[on_module_init]` or
/// `#[on_application_bootstrap]` hook, on HTTP, RPC and gRPC. The call it was built for fails with
/// this event, the way a singleton's hook `Err` fails startup as `StartupError::HookFailed`, and
/// nothing is cached. `kind()` is `Internal`, and the message names the hook alone: the target's
/// path and the hook's error may carry what a caller should not see, so they stay in the fields
/// and the `error` log `DispatchSource::resolve` writes. [`source`](std::error::Error::source)
/// holds the hook's error, and a `#[catch(HookFailed)]` handler can downcast it to reshape the
/// answer.
#[derive(Debug)]
pub struct HookFailed {
    /// The target's DI token.
    pub target: String,
    /// `"on_module_init"` or `"on_application_bootstrap"`.
    pub hook: &'static str,
    pub source: Box<dyn std::error::Error + Send + Sync>,
}

impl HookFailed {
    pub fn new(
        target: impl Into<String>,
        hook: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            target: target.into(),
            hook,
            source,
        }
    }
}

impl fmt::Display for HookFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed", self.hook)
    }
}

impl std::error::Error for HookFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl Error for HookFailed {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Where in the request pipeline a panic was caught. Carried on
/// [`PanicRecovered`] so chain handlers can branch on the site without
/// parsing the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PipelineSegment {
    /// Inside the user handler's own body.
    HandlerBody,
    /// Inside the active transport rendering an error to its wire shape.
    ResponseRendering,
    /// Inside an HTTP middleware's `handle`, which runs before the guards and
    /// interceptors on the matched route.
    Middleware,
    /// Inside an interceptor's `intercept`, on any transport.
    Interceptor,
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
            Self::Interceptor => "interceptor",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::error::render_error;

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
