//! A routing miss, as the global error handlers receive it (transports DESIGN §3.2, §3.3): a
//! `CallError` whose source is one of these.

use std::error::Error;
use std::fmt;

use http::{HeaderValue, Method};
use ulo_transport::{CallError, ErrorKind};

/// The source of the `NotFound` a request matching no route reaches the global error handlers
/// with, which tells it apart from a handler's own `NotFound`. Only the router attaches it.
///
/// ```ignore
/// let error = err.downcast_ref::<CallError>()?;
/// if error.source_as::<NoRoute>().is_some() { /* no route matched */ }
/// if let Some(refused) = error.source_as::<MethodNotAllowed>() { /* refused.allow() */ }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct NoRoute {
    _private: (),
}

impl NoRoute {
    /// `NotFound`, rendered 404 by its kind, with this as its source.
    pub(crate) fn error() -> CallError {
        CallError::new(ErrorKind::NotFound, "no route matches this path").with_source(NoRoute { _private: () })
    }
}

impl fmt::Display for NoRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no route matches this path")
    }
}

impl Error for NoRoute {}

/// A path that matches with no handler for the request's method: the source of the `BadRequest`
/// the global error handlers receive. Unclaimed, and while the error is still of kind
/// `BadRequest`, it renders 405 with [`allow`](Self::allow) as `Allow`, which RFC 9110 requires;
/// an error handler that reshapes it to another kind is rendered by that kind.
///
/// No `ErrorKind` is 405, the status being HTTP's alone. The kind is `BadRequest`, as for an
/// extractor's 413 and 415, so an error handler that renders every error by its kind answers a
/// client error.
///
/// A handler that refuses a method on a path it answers returns one as its error, bare or as the
/// source of a `CallError` carrying its own message, and the error handlers receive it as they
/// receive the router's.
#[derive(Clone, Debug)]
pub struct MethodNotAllowed {
    method: Method,
    allow: HeaderValue,
}

impl MethodNotAllowed {
    /// `method` refused on a path that answers the methods in `allow`, comma-separated.
    pub fn new(method: Method, allow: HeaderValue) -> Self {
        MethodNotAllowed { method, allow }
    }

    /// The method the request used.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The `Allow` value: the methods the path answers, comma-separated, `HEAD` and `OPTIONS`
    /// included when the router answers them. A handler answering the 405 itself sends it as
    /// `Allow`.
    pub fn allow(&self) -> &HeaderValue {
        &self.allow
    }

    /// `BadRequest`, its message this error's text, with this as its source.
    pub(crate) fn into_error(self) -> CallError {
        CallError::new(ErrorKind::BadRequest, self.to_string()).with_source(self)
    }
}

impl fmt::Display for MethodNotAllowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "this path does not answer `{}`", self.method)?;
        if let Ok(allow) = self.allow.to_str() {
            write!(f, "; it answers {allow}")?;
        }
        Ok(())
    }
}

impl Error for MethodNotAllowed {}
