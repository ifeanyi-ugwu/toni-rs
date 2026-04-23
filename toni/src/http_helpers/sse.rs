use std::convert::Infallible;
use std::fmt::Write;

use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;

use super::{Body, HttpResponse, IntoResponse};

/// A single Server-Sent Event.
///
/// Build one with [`SseEvent::data`], then chain optional fields:
///
/// ```rust,ignore
/// SseEvent::data("hello")
///     .event("greet")
///     .id("1")
///     .retry_ms(3000)
/// ```
pub struct SseEvent {
    data: String,
    id: Option<String>,
    event: Option<String>,
    retry: Option<u64>,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            id: None,
            event: None,
            retry: None,
        }
    }

    /// Sets the event's `id` field. The browser sends it back as `Last-Event-ID` on reconnect.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Sets the event type. Clients listen with `es.addEventListener("name", ...)`.
    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    /// Suggests how long (in milliseconds) the browser should wait before reconnecting.
    pub fn retry_ms(mut self, ms: u64) -> Self {
        self.retry = Some(ms);
        self
    }

    fn encode(self) -> Bytes {
        let mut buf = String::new();
        if let Some(id) = self.id {
            writeln!(buf, "id: {id}").unwrap();
        }
        if let Some(event) = self.event {
            writeln!(buf, "event: {event}").unwrap();
        }
        if let Some(ms) = self.retry {
            writeln!(buf, "retry: {ms}").unwrap();
        }
        // SSE spec: multi-line data must be split into separate "data:" lines
        for line in self.data.lines() {
            writeln!(buf, "data: {line}").unwrap();
        }
        if self.data.is_empty() {
            buf.push_str("data: \n");
        }
        buf.push('\n'); // blank line terminates the event
        Bytes::from(buf)
    }
}

/// An SSE response. Wraps a stream of events and sets the required headers.
///
/// Use the [`sse`] free function for infallible streams, or [`Sse::new`] when
/// the stream yields `Result<SseEvent, E>`.
pub struct Sse<S>(S);

impl<S> Sse<S> {
    pub fn new(stream: S) -> Self {
        Self(stream)
    }
}

/// Wraps an infallible stream of [`SseEvent`]s into an SSE response.
///
/// # Example
///
/// ```rust,ignore
/// use futures::stream;
/// use toni::{sse, SseEvent};
///
/// #[get("/events")]
/// async fn events(&self) -> impl IntoResponse {
///     sse(stream::iter([
///         SseEvent::data("hello").event("greet"),
///         SseEvent::data("world").id("2"),
///     ]))
/// }
/// ```
pub fn sse<S>(stream: S) -> Sse<futures::stream::Map<S, fn(SseEvent) -> Result<SseEvent, Infallible>>>
where
    S: Stream<Item = SseEvent>,
{
    Sse::new(stream.map(Ok))
}

impl<S, E> IntoResponse for Sse<S>
where
    S: Stream<Item = Result<SseEvent, E>> + Send + Sync + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn into_response(self) -> HttpResponse {
        let encoded = self.0.map(|r| r.map(SseEvent::encode));
        HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/event-stream".into()),
                ("Cache-Control".into(), "no-cache".into()),
                // Tells nginx/caddy not to buffer the response before forwarding
                ("X-Accel-Buffering".into(), "no".into()),
            ],
            body: Some(Body::stream(encoded)),
        }
    }
}
