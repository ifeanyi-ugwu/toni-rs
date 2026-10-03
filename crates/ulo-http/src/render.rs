//! HTTP's rendering of the one error model (transports DESIGN §2.4, §3.6): RFC 9457
//! `application/problem+json`, `type` `about:blank`, `title` the status phrase, `status` the code,
//! `detail` the public message, `details` as an extension member.

use http::StatusCode;
use ulo::{BoxError, ExecutionRef};
use ulo_transport::{CallError, ErrorKind};

use crate::backend::HttpConfig;
use crate::response::Response;
use crate::sse::Event;

/// The canonical status of a kind. `Timeout` is 504: the server ran out of time, usually waiting
/// downstream; 408 means it gave up waiting for the client's request.
pub(crate) fn status_of(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        ErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Unimplemented => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The problem document for `error`. A 401 carries `WWW-Authenticate`, the error's own challenge or
/// the server's default; a 429 carries `Retry-After` from a `RetryAfter` detail, and a 503 does when
/// one is present.
pub(crate) fn problem(error: &CallError, config: &HttpConfig) -> Response {
    let _ = (error, config);
    todo!("the RFC 9457 document and its headers")
}

/// An error no handler claimed, as a response: `Timeout` when `exec`'s cancel reason is
/// `Deadline`; otherwise `CallError::from_boxed`, with 413 and 415 for an `ExtractError`'s
/// `TooLarge` and `UnsupportedMediaType`.
pub(crate) fn render_error(err: BoxError, exec: &ExecutionRef, config: &HttpConfig) -> Response {
    let _ = (err, exec, config);
    todo!("as documented")
}

/// 503 with `Retry-After` and `Connection: close`: a request arriving once `Execution::open` is
/// refused, during the drain.
pub(crate) fn draining(config: &HttpConfig) -> Response {
    let _ = config;
    todo!("503, `Retry-After: <shed_retry_after>`, `Connection: close`")
}

/// 503 with `Retry-After`: over the server's in-flight bound.
pub(crate) fn shed(config: &HttpConfig) -> Response {
    let _ = config;
    todo!("503, `Retry-After: <shed_retry_after>`")
}

/// An error after an SSE stream began, in its mid-stream form: an event named `error` whose data
/// is the envelope `{"kind", "message", "details"}`, after which the stream ends.
pub(crate) fn late_event(error: &CallError) -> Event {
    let _ = error;
    todo!("the `error` event")
}
