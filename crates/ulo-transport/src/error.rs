//! One error model across transports (transports DESIGN §2.4): a domain error declares a
//! transport-neutral [`ErrorKind`] through [`Classify`], becomes a [`CallError`], and each
//! transport renders that canonically.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use ulo::{BoxError, Closed, FailureReason, GuardRejected, LookupError, PanicRecovered, Redacted, is_panic};

use crate::details::Details;
use crate::extract::ExtractError;

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
    /// The gRPC status code this error is sent with in place of its kind's.
    grpc_code: Option<i32>,
    source: Option<BoxError>,
}

impl CallError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CallError { kind, message: message.into(), details: Details::default(), challenge: None, grpc_code: None, source: None }
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
    /// | `LookupError::Construct { reason: Errored(r) }` | `r` by this table, so a constructor's `CallError` "tenant not found" is 404 |
    /// | `Closed`, `LookupError::Closed` | `Unavailable` |
    /// | anything else | `Internal`, with the message withheld |
    ///
    /// A named constructor rather than `From<BoxError>`: a recogniser that walks an error should
    /// not run invisibly on every `?`. A passed deadline is not in the error: a transport tests
    /// `exec.cancel_reason()` first and renders `Timeout` for `CancelReason::Deadline`.
    ///
    /// The original is kept as the source. A [`Redacted`] holding one of the errors above, as
    /// `err` itself or as a constructor's error inside `LookupError::Construct`, maps as its
    /// original would: redaction changes the text, not the kind. No other `source()` chain is
    /// walked, so an outside error wrapping a `CallError` is `Internal`.
    pub fn from_boxed(err: BoxError) -> Self {
        let err = match err.downcast::<CallError>() {
            Ok(call) => return *call,
            Err(err) => err,
        };
        let err = match err.downcast::<ExtractError>() {
            Ok(extract) => return CallError::from_extract(*extract),
            Err(err) => err,
        };
        let recognised = if is_panic(&err) { None } else { recognise(as_error(&err), 0) };
        let mut call = recognised.unwrap_or_else(internal);
        call.source = Some(err);
        call
    }

    /// The blanket `From`, keeping the challenge of an `Unauthorized` that an execution-scoped
    /// constructor returned behind a `Dependency`, which `Classify` has no method to carry.
    pub(crate) fn from_extract(err: ExtractError) -> Self {
        let challenge = match &err {
            ExtractError::Dependency { source, .. } => from_lookup(source, 0).challenge,
            _ => None,
        };
        let mut call = CallError::from(err);
        call.challenge = challenge;
        call
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

    /// Sends this error over gRPC with the canonical status code `code`, `google.rpc.Code`'s number,
    /// in place of the one its kind maps to. A `Conflict` maps to `ABORTED`, and Google's API
    /// guidance pairs HTTP 409 with `ALREADY_EXISTS` too: an error naming a resource that exists
    /// writes `.with_grpc_code(6)`. The kind still decides every other transport's rendering. The
    /// gRPC transport ignores a code outside 1 to 16, `OK` included.
    ///
    /// A gRPC handler can also return a `tonic::Status` as its error. The error handlers see it as
    /// it was returned, and one none of them claims or replaces is sent unchanged, its code,
    /// message and details as built, with no kind consulted.
    pub fn with_grpc_code(mut self, code: i32) -> Self {
        self.grpc_code = Some(code);
        self
    }

    /// The gRPC status code [`with_grpc_code`](Self::with_grpc_code) set.
    pub fn grpc_code(&self) -> Option<i32> {
        self.grpc_code
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
            grpc_code: self.grpc_code,
            source: None,
        }
    }
}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        let mut call = classified(&e);
        call.source = Some(Box::new(e));
        call
    }
}

pub(crate) const INTERNAL_MESSAGE: &str = "internal error";
const FORBIDDEN_MESSAGE: &str = "forbidden";
const CLOSED_MESSAGE: &str = "the server is shutting down";

/// Bounded, in case a chain of `Redacted` and constructor errors loops back on itself.
const MAX_DEPTH: usize = 32;

/// `Internal` with the message withheld: the original may name keys, types or internals.
fn internal() -> CallError {
    CallError::new(ErrorKind::Internal, INTERNAL_MESSAGE)
}

fn classified<E: Classify + ?Sized>(e: &E) -> CallError {
    CallError::new(e.classify(), e.public_message().into_owned()).with_details(e.details())
}

fn as_error(err: &BoxError) -> &(dyn Error + 'static) {
    &**err
}

/// A source-less `CallError` for one of the errors `from_boxed` recognises, or `None` for any
/// other type.
fn recognise<'e>(error: impl ByType<'e>, depth: usize) -> Option<CallError> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(call) = error.get::<CallError>() {
        return Some(call.summary());
    }
    if let Some(extract) = error.get::<ExtractError>() {
        let mut call = classified(extract);
        if let ExtractError::Dependency { source, .. } = extract {
            call.challenge = from_lookup(source, depth + 1).challenge;
        }
        return Some(call);
    }
    if error.get::<PanicRecovered>().is_some() {
        return Some(internal());
    }
    if error.get::<GuardRejected>().is_some() {
        return Some(CallError::new(ErrorKind::Forbidden, FORBIDDEN_MESSAGE));
    }
    if let Some(lookup) = error.get::<LookupError>() {
        return Some(from_lookup(lookup, depth + 1));
    }
    if error.get::<Closed>().is_some() {
        return Some(CallError::new(ErrorKind::Unavailable, CLOSED_MESSAGE));
    }
    if let Some(redacted) = error.get::<Redacted>() {
        return recognise(redacted, depth + 1);
    }
    None
}

/// A lookup error's kind: a constructor's own error maps as `from_boxed` maps it, so an
/// execution-scoped constructor's "tenant not found" is a 404; a lookup from Destroying on is
/// `Unavailable`; every other lookup failure, a panic or timeout inside the build included, is
/// `Internal` with the message withheld.
pub(crate) fn from_lookup(lookup: &LookupError, depth: usize) -> CallError {
    match lookup {
        LookupError::Construct { reason: FailureReason::Errored(cause), .. } => {
            recognise(cause, depth + 1).unwrap_or_else(internal)
        }
        LookupError::Closed { .. } => CallError::new(ErrorKind::Unavailable, CLOSED_MESSAGE),
        _ => internal(),
    }
}

/// The original by its type, which `dyn Error` and `Redacted` both answer.
trait ByType<'e>: Copy {
    fn get<E: Error + 'static>(self) -> Option<&'e E>;
}

impl<'e> ByType<'e> for &'e (dyn Error + 'static) {
    fn get<E: Error + 'static>(self) -> Option<&'e E> {
        self.downcast_ref::<E>()
    }
}

impl<'e> ByType<'e> for &'e Redacted {
    fn get<E: Error + 'static>(self) -> Option<&'e E> {
        self.downcast_ref::<E>()
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
