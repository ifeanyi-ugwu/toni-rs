//! Server-Sent Events, following the WHATWG HTML specification (transports DESIGN §3.1, §2.6).

use std::error::Error;
use std::fmt;
use std::time::Duration;

use futures_core::Stream;
use serde::Serialize;
use ulo_transport::{CallError, IntoReply, IntoReplyError};

use crate::cx::HttpCx;
use crate::response::Response;
use crate::transport::Http;

/// A stream of events as a response: status 200, `text/event-stream` in UTF-8.
///
/// `dispatch` returns the response without pulling the stream, so the headers are never held back
/// for the first event. An `Err` item, the first included, takes the late path
/// (`ulo::dispatch_late`): the error handlers may reshape it or end the stream with
/// `Err(EndStream)`, and it is written as an `error` event, since a status can no longer change.
/// The stream is wrapped in `Tracked`, so an `on_stream_end` callback learns whether it completed
/// or was cut off.
///
/// ```ignore
/// Sse::new(self.feed.since(resume).take_until(exec.draining()))
///     .keep_alive(Duration::from_secs(15))
/// ```
pub struct Sse<S> {
    stream: S,
    keep_alive: Option<Duration>,
    end_event: Option<&'static str>,
}

impl<S> Sse<S> {
    pub fn new(stream: S) -> Self {
        Sse { stream, keep_alive: None, end_event: None }
    }

    /// A `: keepalive` comment whenever `every` passes without an event, so an idle stream is not
    /// closed by an intermediary.
    pub fn keep_alive(self, every: Duration) -> Self {
        Sse { keep_alive: Some(every), ..self }
    }

    /// A final event named `name`, with empty data, written when the stream returns `None`. SSE has
    /// no end marker in its specification, so a client cannot tell a clean end from a cut-off
    /// without one; off by default for that reason. A `name` that is not a valid event name fails
    /// the reply before anything is written.
    pub fn end_event(self, name: &'static str) -> Self {
        Sse { end_event: Some(name), ..self }
    }
}

/// One event. `data` is split into `data:` lines at CR, LF and CRLF alike, since a reader ends a
/// line at any of the three.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub(crate) data: Option<String>,
    pub(crate) id: Option<EventId>,
    pub(crate) event: Option<EventName>,
    pub(crate) retry: Option<Duration>,
    pub(crate) comment: Option<String>,
}

impl Event {
    pub fn data(self, data: impl Into<String>) -> Self {
        Event { data: Some(data.into()), ..self }
    }

    /// `data` as the JSON of `value`.
    pub fn json<T: Serialize>(self, value: &T) -> Result<Self, serde_json::Error> {
        Ok(Event { data: Some(serde_json::to_string(value)?), ..self })
    }

    pub fn id(self, id: EventId) -> Self {
        Event { id: Some(id), ..self }
    }

    pub fn event(self, name: EventName) -> Self {
        Event { event: Some(name), ..self }
    }

    /// The reconnection time a client should use, written in whole milliseconds.
    pub fn retry(self, after: Duration) -> Self {
        Event { retry: Some(after), ..self }
    }

    /// A comment line; a reader raises no event for a comment-only frame. Line terminators in
    /// `text` start a new comment line.
    pub fn comment(self, text: impl Into<String>) -> Self {
        Event { comment: Some(text.into()), ..self }
    }

    /// The event's wire form, ending with the blank line that dispatches it.
    pub(crate) fn encode(&self) -> String {
        todo!("comment, event, id, retry, then data lines split at CR, LF and CRLF, then a blank line")
    }
}

/// An event's `id`, refusing a line terminator and U+0000, which a reader ignores. Built
/// fallibly, so a bad value fails where it is built and never mid-stream. A reconnecting client
/// returns it as `Last-Event-ID`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EventId(String);

impl EventId {
    pub fn new(id: impl Into<String>) -> Result<Self, EventFieldError> {
        let id = id.into();
        if id.contains(['\r', '\n']) {
            return Err(EventFieldError { field: "id", reason: "a line terminator" });
        }
        if id.contains('\0') {
            return Err(EventFieldError { field: "id", reason: "U+0000" });
        }
        Ok(EventId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An event's `event` name, refusing a line terminator.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EventName(String);

impl EventName {
    pub fn new(name: impl Into<String>) -> Result<Self, EventFieldError> {
        let name = name.into();
        if name.contains(['\r', '\n']) {
            return Err(EventFieldError { field: "event", reason: "a line terminator" });
        }
        Ok(EventName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `EventId::new` or `EventName::new` refused a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventFieldError {
    field: &'static str,
    reason: &'static str,
}

impl fmt::Display for EventFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an SSE `{}` field cannot hold {}", self.field, self.reason)
    }
}

impl Error for EventFieldError {}

/// An item an `Sse` stream yields: an `Event`, or a `Result` whose error converts into a
/// `CallError`, a `Classify` error among them, so its kind survives into the error handlers on the
/// late path.
pub trait SseItem: Send + 'static {
    fn into_event(self) -> Result<Event, CallError>;
}

impl SseItem for Event {
    fn into_event(self) -> Result<Event, CallError> {
        Ok(self)
    }
}

impl<E: Into<CallError> + Send + 'static> SseItem for Result<Event, E> {
    fn into_event(self) -> Result<Event, CallError> {
        self.map_err(Into::into)
    }
}

impl<S> IntoReply<Http> for Sse<S>
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = (cx, self.stream, self.keep_alive, self.end_event);
        todo!("validate `end_event`; a body encoding each event, keep-alive comments on a tokio interval, item errors through `dispatch_late` with the matched handler, wrapped in `Tracked`")
    }
}
