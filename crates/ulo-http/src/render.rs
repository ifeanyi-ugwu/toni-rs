//! HTTP's rendering of the one error model (transports DESIGN §2.4, §3.6): RFC 9457
//! `application/problem+json`, `type` `about:blank`, `title` the status phrase, `status` the code,
//! `detail` the public message, `details` as an extension member.

use std::error::Error;
use std::time::Duration;

use http::header::{ACCEPT, CONNECTION, CONTENT_TYPE, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
use http::StatusCode;
use serde::Serialize;
use ulo::{BoxError, CancelReason, ExecutionRef};
use ulo_transport::{CallError, Details, ErrorKind, ExtractError};

use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::response::Response;
use crate::sse::{Event, EventName};

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
    document(error, status_of(error.kind()), config)
}

/// An error no handler claimed, as a response: `Timeout` when `exec`'s cancel reason is
/// `Deadline`; otherwise `CallError::from_boxed`, with 413 and 415 for an `ExtractError`'s
/// `TooLarge` and `UnsupportedMediaType`.
///
/// The 413 and 415 apply to an `ExtractError` arriving bare, as extraction raises it, or as the
/// source of a `CallError` still of its kind, `BadRequest`; an error handler that reshaped it to
/// another kind is rendered by that kind. A 415 carries `Accept` with the media type the
/// extractor expected (RFC 9110 §15.5.16).
pub(crate) fn render_error(err: BoxError, exec: &ExecutionRef, config: &HttpConfig) -> Response {
    if exec.cancel_reason() == Some(CancelReason::Deadline) {
        return problem(&timed_out(), config);
    }
    let own_status = body_refusal(&*err);
    let error = CallError::from_boxed(err);
    match own_status {
        Some((status, expected)) if error.kind() == ErrorKind::BadRequest => {
            let mut response = document(&error, status, config);
            if let Some(accept) = expected.and_then(|expected| HeaderValue::from_str(expected).ok()) {
                response.headers_mut().insert(ACCEPT, accept);
            }
            response
        }
        _ => problem(&error, config),
    }
}

/// 503 with `Retry-After` and `Connection: close`: a request arriving once `Execution::open` is
/// refused, during the drain.
pub(crate) fn draining(config: &HttpConfig) -> Response {
    let error = CallError::new(ErrorKind::Unavailable, "the server is shutting down");
    let mut response = refusal(&error, config);
    response.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
    response
}

/// 503 with `Retry-After`: over the server's in-flight bound.
pub(crate) fn shed(config: &HttpConfig) -> Response {
    refusal(&CallError::new(ErrorKind::Unavailable, "the server is at its limit of requests in flight"), config)
}

/// An error after an SSE stream began, in its mid-stream form: an event named `error` whose data
/// is the envelope `{"kind", "message", "details"}`, after which the stream ends.
pub(crate) fn late_event(error: &CallError) -> Event {
    let envelope = Envelope { kind: error.kind().as_str(), message: error.message(), details: error.details() };
    let data = serde_json::to_string(&envelope).unwrap_or_else(|failure| {
        tracing::warn!(error = %failure, "the details of an SSE error event could not be encoded; they are left out");
        let bare = Details::default();
        let envelope = Envelope { details: &bare, ..envelope };
        serde_json::to_string(&envelope).unwrap_or_default()
    });
    Event::default().event(EventName::error()).data(data)
}

/// `Timeout`, for an execution cancelled by its deadline: the error it ends with is whatever the
/// cancellation interrupted, and the reason lives on the execution.
pub(crate) fn timed_out() -> CallError {
    CallError::new(ErrorKind::Timeout, "the request did not complete within its time limit")
}

/// An `ExtractError` that has its own status: 413 for `TooLarge`, 415 with the expected media type
/// for `UnsupportedMediaType`.
fn body_refusal(err: &(dyn Error + Send + Sync + 'static)) -> Option<(StatusCode, Option<&'static str>)> {
    let extract = err
        .downcast_ref::<ExtractError>()
        .or_else(|| err.downcast_ref::<CallError>().and_then(|error| error.source_as::<ExtractError>()))?;
    match extract {
        ExtractError::TooLarge { .. } => Some((StatusCode::PAYLOAD_TOO_LARGE, None)),
        ExtractError::UnsupportedMediaType { expected, .. } => Some((StatusCode::UNSUPPORTED_MEDIA_TYPE, Some(*expected))),
        _ => None,
    }
}

/// A 503 that always carries `Retry-After`, the server's `shed_retry_after`.
fn refusal(error: &CallError, config: &HttpConfig) -> Response {
    let mut response = document(error, StatusCode::SERVICE_UNAVAILABLE, config);
    response.headers_mut().insert(RETRY_AFTER, retry_after(config.shed_retry_after));
    response
}

fn document(error: &CallError, status: StatusCode, config: &HttpConfig) -> Response {
    let problem = Problem { kind: "about:blank", title: title(status), status: status.as_u16(), detail: error.message(), details: error.details() };
    let body = serde_json::to_vec(&problem).unwrap_or_else(|failure| {
        tracing::warn!(error = %failure, "the details of a problem document could not be encoded; they are left out");
        let bare = Details::default();
        serde_json::to_vec(&Problem { details: &bare, ..problem }).unwrap_or_default()
    });
    let mut response = Response::new(HttpBody::from_bytes(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
    if status == StatusCode::UNAUTHORIZED {
        let challenge = error.challenge().unwrap_or(&*config.challenge);
        let value = HeaderValue::from_str(challenge).unwrap_or_else(|_| {
            tracing::warn!(challenge, "a `WWW-Authenticate` challenge is not a valid header value; `Bearer` is sent instead");
            HeaderValue::from_static("Bearer")
        });
        headers.insert(WWW_AUTHENTICATE, value);
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        if let Some(after) = error.details().retry_after() {
            headers.insert(RETRY_AFTER, retry_after(after));
        }
    }
    response
}

/// `Retry-After` as delay-seconds (RFC 9110 §10.2.3), a fraction of a second rounded up so a client
/// never retries early.
fn retry_after(after: Duration) -> HeaderValue {
    let seconds = after.as_secs() + u64::from(after.subsec_nanos() > 0);
    HeaderValue::from(seconds)
}

/// The status phrase RFC 9110 recommends, which RFC 9457 makes the `title` of an `about:blank`
/// problem. The `http` crate's phrases predate RFC 9110 for 413 and 422.
fn title(status: StatusCode) -> &'static str {
    match status.as_u16() {
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        _ => status.canonical_reason().unwrap_or(""),
    }
}

#[derive(Clone, Copy, Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    detail: &'a str,
    #[serde(skip_serializing_if = "no_details")]
    details: &'a Details,
}

#[derive(Clone, Copy, Serialize)]
struct Envelope<'a> {
    kind: &'static str,
    message: &'a str,
    details: &'a Details,
}

fn no_details(details: &&Details) -> bool {
    details.is_empty()
}
