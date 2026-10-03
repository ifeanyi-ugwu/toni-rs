//! Handler parameters built from the call (transports DESIGN §2.2).

use std::error::Error;
use std::fmt;
use std::future::Future;

use ulo::{Dependencies, ExecutionRef, FromContainer, LookupError, Redacted, Transport};

use crate::details::FieldViolation;
use crate::error::{Classify, ErrorKind};

/// A handler parameter built from the call.
///
/// The pair `FromContainer` / `FromCall` names where a parameter comes from, the container or the
/// call, and both appear in diagnostics. A transport implements `FromCall` for its own
/// extractors; a `FromContainer` type (`Dep`, `Many`, `Ext`, `ModuleRef`, `ExecutionRef`, a user
/// type) is accepted bare on every transport, the handler attribute reading it through the
/// container.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be built from a `{T}` call",
    note = "a transport implements `FromCall` for its own extractors; `FromContainer` types (`Dep`, `Ext`, ...) are accepted on every transport"
)]
pub trait FromCall<T: Transport>: Sized + Send + 'static {
    /// Whether this parameter consumes the body or payload. At most one may: a handler's
    /// attribute asserts it for every pair of parameters at compile time, naming both.
    const CONSUMES_BODY: bool = false;

    /// What it reads from the container, if anything, for the wiring pass.
    fn dependencies(_d: &mut Dependencies) {}

    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send;
}

/// `None` exactly where `P` fails with [`ExtractError::Missing`]: a header, query field or body
/// the call does not carry. Every other failure propagates. Forwards `CONSUMES_BODY` and the
/// dependencies.
impl<T: Transport, P: FromCall<T>> FromCall<T> for Option<P> {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;

    fn dependencies(d: &mut Dependencies) {
        P::dependencies(d);
    }

    async fn from_call(cx: &T::Cx) -> Result<Self, ExtractError> {
        match P::from_call(cx).await {
            Ok(value) => Ok(Some(value)),
            Err(ExtractError::Missing { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }
}

/// A `FromContainer` type as a `FromCall` parameter, read through the call's execution with the
/// controller's module visibility. A handler attribute accepts the type bare and reads it this
/// way; the wrapper is for code generic over `FromCall`.
pub struct Injected<S>(pub S);

impl<T, S> FromCall<T> for Injected<S>
where
    T: Transport,
    T::Cx: AsRef<ExecutionRef>,
    S: FromContainer,
{
    fn dependencies(d: &mut Dependencies) {
        d.add::<S>();
    }

    async fn from_call(cx: &T::Cx) -> Result<Self, ExtractError> {
        let exec: &ExecutionRef = cx.as_ref();
        let resolver = exec.resolver();
        match S::from_container(&resolver).await {
            Ok(value) => Ok(Injected(value)),
            Err(source) => Err(ExtractError::Dependency { param: std::any::type_name::<S>(), source }),
        }
    }
}

/// An extraction failure, one shape on every transport, each variant carrying the name of what
/// failed so one pattern matches it: the path segment, query field, header, `"body"`, or the
/// container type.
///
/// It reaches the error handlers as a `CallError` of its kind, holding the `ExtractError` as its
/// source; an error handler reshapes it with
/// `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()`. Each transport documents its
/// rendering; HTTP answers 413 for `TooLarge` and 415 for `UnsupportedMediaType`.
#[non_exhaustive]
#[derive(Debug)]
pub enum ExtractError {
    /// A required header, query field or path segment.
    Missing { param: &'static str },
    /// Could not decode. `source` is built through `AppHandle::redact`, so a decoder's error
    /// echoing a credential is redacted like any outside error.
    Malformed { param: &'static str, source: Redacted },
    UnsupportedMediaType { param: &'static str, expected: &'static str },
    TooLarge { param: &'static str, limit: u64 },
    /// Validation rules failed.
    Invalid { param: &'static str, violations: Vec<FieldViolation> },
    /// A `FromContainer` parameter's read failed: `param` is the type's name, `source` the
    /// lookup error, which classifies as `CallError::from_boxed` maps it, so an execution-scoped
    /// constructor's "tenant not found" is a 404 here too.
    Dependency { param: &'static str, source: LookupError },
}

impl ExtractError {
    pub fn param(&self) -> &'static str {
        match self {
            ExtractError::Missing { param }
            | ExtractError::Malformed { param, .. }
            | ExtractError::UnsupportedMediaType { param, .. }
            | ExtractError::TooLarge { param, .. }
            | ExtractError::Invalid { param, .. }
            | ExtractError::Dependency { param, .. } => param,
        }
    }
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::Missing { param } => write!(f, "`{param}` is missing"),
            ExtractError::Malformed { param, source } => write!(f, "`{param}` could not be decoded: {source}"),
            ExtractError::UnsupportedMediaType { param, expected } => write!(f, "`{param}` must be `{expected}`"),
            ExtractError::TooLarge { param, limit } => write!(f, "`{param}` is larger than {limit} bytes"),
            ExtractError::Invalid { param, violations } => {
                let count = violations.len();
                let noun = if count == 1 { "rule" } else { "rules" };
                write!(f, "`{param}` failed {count} validation {noun}")
            }
            ExtractError::Dependency { source, .. } => fmt::Display::fmt(source, f),
        }
    }
}

impl Error for ExtractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExtractError::Dependency { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// `BadRequest`, or `Unprocessable` for `Invalid` with its violations as a `FieldViolations`
/// detail; a `Dependency` takes the kind its lookup error maps to.
impl Classify for ExtractError {
    fn classify(&self) -> ErrorKind {
        todo!("`Invalid` → Unprocessable; `Dependency` → the lookup error's kind as `CallError::from_boxed` maps it; the rest BadRequest")
    }

    fn details(&self) -> crate::details::Details {
        todo!("`Invalid` → one `Detail::FieldViolations`; the rest none")
    }
}
