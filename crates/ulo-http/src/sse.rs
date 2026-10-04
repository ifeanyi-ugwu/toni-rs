//! Server-Sent Events, following the WHATWG HTML specification (transports DESIGN §3.1, §2.6).

use std::error::Error;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use serde::Serialize;
use ulo::{BoxError, BoxFuture, CancelReason, LateOutcome, MountedHandler, StreamOutcome, Timer};
use ulo_transport::{CallError, IntoReply, IntoReplyError, Tracked};

use crate::body::HttpBody;
use crate::cx::HttpCx;
use crate::render;
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

    /// A `: keepalive` comment after at least `every` of idleness, so an intermediary does not
    /// close the stream as idle. The period starts when the stream is next found waiting after a
    /// write, so a comment comes `every` after the last write or a little later, never sooner.
    /// Choose a period comfortably below the shortest proxy idle timeout in the deployment.
    ///
    /// The period is timed by the app's `Timer`, which `HttpCx::timer` reads.
    ///
    /// `Duration::ZERO` turns the keep-alive off, as leaving `keep_alive` uncalled does: a zero
    /// period would write a comment every time the stream is found waiting.
    pub fn keep_alive(self, every: Duration) -> Self {
        Sse { keep_alive: Some(every).filter(|every| !every.is_zero()), ..self }
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

    /// The event's wire form, ending with the blank line that dispatches it: comment lines, then
    /// `event`, `id`, `retry`, then the data lines.
    ///
    /// Every field is written as `name: value`; a reader drops the one space after the colon, so a
    /// value keeps a leading space of its own. Data keeps the piece after a trailing terminator:
    /// `"a\n"` is written as `data: a` and `data: `, which a reader joins back into `"a\n"`. Empty
    /// data is one `data: ` line, which a reader dispatches as an event carrying `""`; an event
    /// without data has no `data` line, and a reader dispatches nothing for it.
    pub(crate) fn encode(&self) -> String {
        let mut out = String::new();
        if let Some(comment) = &self.comment {
            for line in lines(comment) {
                field(&mut out, "", line);
            }
        }
        if let Some(name) = &self.event {
            field(&mut out, "event", name.as_str());
        }
        if let Some(id) = &self.id {
            field(&mut out, "id", id.as_str());
        }
        if let Some(retry) = self.retry {
            field(&mut out, "retry", &retry.as_millis().to_string());
        }
        if let Some(data) = &self.data {
            for line in lines(data) {
                field(&mut out, "data", line);
            }
        }
        out.push('\n');
        out
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

    /// `error`, the name of the event an error after the stream began is written as.
    pub(crate) fn error() -> Self {
        EventName(String::from("error"))
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

/// Status 200, `text/event-stream; charset=utf-8`, `Cache-Control: no-cache`, the events written as
/// the stream yields them.
///
/// A keep-alive comment is written once the stream has been idle for at least the keep-alive
/// period, timed by the app's `Timer`, the clock starting again after every event; while the
/// stream yields events back to back none is written. An `Err` item runs the matched handler's error handlers
/// through `ulo::dispatch_late`; unless they end the stream with `EndStream`, the error is written
/// as an `error` event, the stream reports `StreamOutcome::CutOff(None)` and then ends.
impl<S> IntoReply<Http> for Sse<S>
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let end_event = self.end_event.map(|name| EventName::new(name)).transpose().map_err(IntoReplyError::new)?;
        let body = SseBody {
            stream: Box::pin(self.stream),
            keep_alive: self.keep_alive.map(|every| KeepAlive { every, timer: Arc::clone(cx.timer()), sleep: None }),
            end_event,
            handler: cx.matched().map(|route| route.handler.clone()),
            cx: cx.clone(),
            state: State::Streaming,
        };
        let mut response = Response::new(HttpBody::stream(Tracked::new(body, cx.exec().clone())));
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        Ok(response)
    }
}

/// The lines of `text`, split at CR, LF and CRLF alike, keeping the piece after a trailing
/// terminator.
fn lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\r' => {
                lines.push(&text[start..at]);
                if bytes.get(at + 1) == Some(&b'\n') {
                    at += 1;
                }
                start = at + 1;
            }
            b'\n' => {
                lines.push(&text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
        at += 1;
    }
    lines.push(&text[start..]);
    lines
}

/// One `name: value` line; an empty name writes a comment.
fn field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

const KEEP_ALIVE: &[u8] = b": keepalive\n\n";

/// The response body of an `Sse`: the stream's events encoded, keep-alive comments between them,
/// and the late path for an `Err` item.
struct SseBody<S> {
    stream: Pin<Box<S>>,
    keep_alive: Option<KeepAlive>,
    end_event: Option<EventName>,
    /// The matched route's handler, whose error handlers an `Err` item reaches. `None` for an
    /// `Sse` an error handler answered a miss with, whose item errors are written unreshaped.
    handler: Option<MountedHandler<Http>>,
    /// Held while the stream runs, which keeps the execution's instances and its cancellation
    /// signal alive.
    cx: HttpCx,
    state: State,
}

/// The keep-alive clock, read from the app's `Timer`.
struct KeepAlive {
    every: Duration,
    timer: Arc<dyn Timer>,
    /// The period running: started by the first poll that finds the stream pending after
    /// something was written, and dropped when something is written.
    sleep: Option<BoxFuture<'static, ()>>,
}

enum State {
    Streaming,
    /// An `Err` item in the error handlers, with the canonical form of the original kept for an
    /// error handler answering `Ok`.
    Late { original: CallError, outcome: BoxFuture<'static, LateOutcome> },
    /// The stream ended cleanly: the end event, if one is set, is written next.
    Ending,
    Done,
}

impl KeepAlive {
    /// Whether the period passed with nothing written. A sleep is requested from the `Timer` only
    /// once the stream is found pending, so events written back to back request none.
    fn poll_due(&mut self, cx: &mut Context<'_>) -> bool {
        let timer = &self.timer;
        let every = self.every;
        let sleep = self.sleep.get_or_insert_with(|| timer.sleep(every));
        if sleep.as_mut().poll(cx).is_pending() {
            return false;
        }
        self.sleep = None;
        true
    }

    /// Something was written: the period starts again at the next pending poll.
    fn restart(&mut self) {
        self.sleep = None;
    }
}

impl<S> SseBody<S>
where
    S: Stream,
    S::Item: SseItem,
{
    /// The `error` event for a late error, `Timeout` when the execution's deadline cancelled it.
    fn late(&self, error: CallError) -> Bytes {
        let error = if self.cx.exec().cancel_reason() == Some(CancelReason::Deadline) { render::timed_out() } else { error };
        self.cx.exec().report_stream_end(StreamOutcome::CutOff(None));
        Bytes::from(render::late_event(&error).encode())
    }

    fn written(&mut self, bytes: Bytes) -> Poll<Option<Result<Bytes, BoxError>>> {
        if let Some(keep_alive) = &mut self.keep_alive {
            keep_alive.restart();
        }
        Poll::Ready(Some(Ok(bytes)))
    }
}

impl<S> Stream for SseBody<S>
where
    S: Stream,
    S::Item: SseItem,
{
    type Item = Result<Bytes, BoxError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                State::Streaming => match this.stream.as_mut().poll_next(cx) {
                    Poll::Ready(Some(item)) => match item.into_event() {
                        Ok(event) => return this.written(Bytes::from(event.encode())),
                        Err(error) => {
                            let original = error.summary();
                            match &this.handler {
                                Some(handler) => {
                                    let handler = handler.clone();
                                    let exec = this.cx.exec().clone();
                                    let call_cx = this.cx.clone();
                                    let outcome: BoxFuture<'static, LateOutcome> =
                                        Box::pin(async move { ulo::dispatch_late(&handler, &exec, &call_cx, BoxError::from(error)).await });
                                    this.state = State::Late { original, outcome };
                                }
                                None => {
                                    this.state = State::Done;
                                    let bytes = this.late(error);
                                    return this.written(bytes);
                                }
                            }
                        }
                    },
                    Poll::Ready(None) => this.state = State::Ending,
                    Poll::Pending => {
                        if this.keep_alive.as_mut().is_some_and(|keep_alive| keep_alive.poll_due(cx)) {
                            return Poll::Ready(Some(Ok(Bytes::from_static(KEEP_ALIVE))));
                        }
                        return Poll::Pending;
                    }
                },
                State::Late { original, outcome } => {
                    let error = match ready!(outcome.as_mut().poll(cx)) {
                        LateOutcome::End => {
                            this.state = State::Ending;
                            continue;
                        }
                        LateOutcome::Render(error) => CallError::from_boxed(error),
                        LateOutcome::Ignored => {
                            tracing::warn!(
                                handler = this.handler.as_ref().map(|handler| handler.name()),
                                "an error handler answered `Ok` to an error raised after the event stream began; its reply is dropped and the original error is written"
                            );
                            original.summary()
                        }
                        _ => original.summary(),
                    };
                    this.state = State::Done;
                    let bytes = this.late(error);
                    return this.written(bytes);
                }
                State::Ending => {
                    this.state = State::Done;
                    if let Some(name) = this.end_event.take() {
                        let bytes = Bytes::from(Event::default().event(name).data("").encode());
                        return this.written(bytes);
                    }
                }
                State::Done => return Poll::Ready(None),
            }
        }
    }
}
