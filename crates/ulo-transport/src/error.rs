//! One error model across transports (transports DESIGN §2.4): a domain error declares a
//! transport-neutral [`ErrorKind`] through [`Classify`], becomes a [`CallError`], and each
//! transport renders that canonically.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use ulo::BoxError;

use crate::details::Details;

/// The transport-neutral kind of a failure, which each transport maps to its own status: HTTP
/// status codes, gRPC canonical codes, the WebSocket and RPC envelope's `kind` string.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Unprocessable,
    TooManyRequests,
    Timeout,
    Unavailable,
    Unimplemented,
    Internal,
}

impl ErrorKind {
    /// The WebSocket and RPC envelope's `kind`: `"bad_request"`, `"not_found"`, ..
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Unprocessable => "unprocessable",
            ErrorKind::TooManyRequests => "too_many_requests",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Unimplemented => "unimplemented",
            ErrorKind::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A domain error that knows its transport-neutral kind.
///
/// `#[derive(Classify)]` with `#[classify(not_found)]` on the type or on each variant writes the
/// impl beside thiserror's `#[error(..)]`. Implementing it is what makes `?` work inside a
/// function returning `Result<_, CallError>`: the blanket `impl<E: Classify> From<E> for
/// CallError` converts the error and keeps it as the source.
///
/// [`CallError`] never implements `Classify`, and must not: with that impl the blanket `From`
/// would collide with std's `impl<T> From<T> for T` (E0119). That is why `CallError` carries an
/// inherent [`kind`](CallError::kind) instead.
pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;

    /// What the caller may read. Defaults to the error's `Display`; override it for an error
    /// whose text names internals.
    fn public_message(&self) -> Cow<'_, str> {
        self.to_string().into()
    }

    fn details(&self) -> Details {
        Details::default()
    }
}

/// The one concrete error every transport renders: a kind, a public message, typed details, and
/// the domain error it came from.
pub struct CallError {
    kind: ErrorKind,
    message: String,
    details: Details,
    /// The `WWW-Authenticate` challenge an `Unauthorized` carries, overriding the server's default.
    challenge: Option<String>,
    source: Option<BoxError>,
}

impl CallError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CallError { kind, message: message.into(), details: Details::default(), challenge: None, source: None }
    }

    /// `Unauthorized`, carrying its own `WWW-Authenticate` challenge (RFC 9110 requires one on a
    /// 401); without one the HTTP server's configured default applies, `Bearer` unless set.
    pub fn unauthorized(challenge: impl Into<String>) -> Self {
        let mut error = CallError::new(ErrorKind::Unauthorized, "unauthorized");
        error.challenge = Some(challenge.into());
        error
    }

    /// The recogniser: walks `err` and maps the core's own errors to a kind.
    ///
    /// | Error | Kind |
    /// |---|---|
    /// | `CallError`, bare or boxed by the reply probe | its own |
    /// | `ExtractError` | `BadRequest` or `Unprocessable`; a `Dependency` as its lookup error maps |
    /// | `GuardRejected` | `Forbidden` |
    /// | `PanicRecovered`, and anything `ulo::is_panic` recognises | `Internal`, with a generic message |
    /// | `LookupError::Construct { reason: Errored(r) }` | `r.downcast_ref::<CallError>()`'s kind, so a constructor's "tenant not found" is 404 |
    /// | `Closed`, `LookupError::Closed` | `Unavailable` |
    /// | anything else | `Internal`, with the message withheld |
    ///
    /// A named constructor rather than `From<BoxError>`: a recogniser that walks an error should
    /// not run invisibly on every `?`. A passed deadline is not in the error: a transport tests
    /// `exec.cancel_reason()` first and renders `Timeout` for `CancelReason::Deadline`.
    pub fn from_boxed(err: BoxError) -> Self {
        let _ = err;
        todo!("the table above; the original kept as the source")
    }

    pub fn with_details(mut self, details: Details) -> Self {
        self.details = details;
        self
    }

    /// Keeps `source` as the error an error handler reaches through [`source_as`](Self::source_as).
    pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The public message: what a transport writes as `detail` or `message`.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &Details {
        &self.details
    }

    pub fn challenge(&self) -> Option<&str> {
        self.challenge.as_deref()
    }

    /// The domain error this was built from, by type: `err.downcast_ref::<CallError>()?.source_as::<UserError>()`.
    pub fn source_as<E: Error + 'static>(&self) -> Option<&E> {
        self.source.as_deref().and_then(|source| source.downcast_ref::<E>())
    }

    /// A copy without the source: kind, message, details and challenge. What a transport keeps
    /// before handing the error to `ulo::dispatch_late`, which takes it by value, so it can still
    /// render the original when an error handler answers `Ok` on the late path.
    pub fn summary(&self) -> CallError {
        CallError {
            kind: self.kind,
            message: self.message.clone(),
            details: self.details.clone(),
            challenge: self.challenge.clone(),
            source: None,
        }
    }
}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        CallError {
            kind: e.classify(),
            message: e.public_message().into_owned(),
            details: e.details(),
            challenge: None,
            source: Some(Box::new(e)),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}

impl Error for CallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|source| source as &(dyn Error + 'static))
    }
}
