use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use http_body::{Body as _, Frame, SizeHint};
use http_body_util::combinators::UnsyncBoxBody;
use ulo::{BoxError, ExecutionRef};
use ulo_transport::Tracked;

/// The one body type of requests and responses: a stream of `Bytes` frames, the `http_body::Body`
/// every backend converts to and from at its edge.
///
/// Dropping a request body unread is not a disconnect: `CancelReason::Disconnected` fires only when
/// the backend observes the peer close the connection or reset the stream.
pub struct HttpBody {
    inner: UnsyncBoxBody<Bytes, BoxError>,
    /// A `Tracked` inside reports this body's end, so the service does not wrap it again. A body
    /// rebuilt around this one through [`HttpBody::new`] starts unmarked and is wrapped again, and
    /// its end is reported once, the first report winning.
    tracked: bool,
}

/// `HttpBody` under the name a response reads with: `Body::stream(s)`.
pub type Body = HttpBody;

impl HttpBody {
    pub fn empty() -> Self {
        HttpBody::new(http_body_util::Empty::<Bytes>::new())
    }

    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        HttpBody::new(http_body_util::Full::new(bytes.into()))
    }

    /// A streaming body: chunked encoding on HTTP/1.1, data frames on HTTP/2. The service wraps it
    /// in `ulo_transport::Tracked` as it answers, however the response holding it was built, so
    /// `on_stream_end` callbacks learn how it ended.
    pub fn stream<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Into<BoxError>,
    {
        use futures_util::StreamExt;
        let frames = stream.map(|chunk: Result<Bytes, E>| -> Result<Frame<Bytes>, BoxError> { chunk.map(Frame::data).map_err(Into::into) });
        HttpBody::new(http_body_util::StreamBody::new(frames))
    }

    /// Any `http_body::Body` of `Bytes`, its error boxed.
    pub fn new<B>(body: B) -> Self
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        use http_body_util::BodyExt;
        HttpBody { inner: body.map_err(Into::into).boxed_unsync(), tracked: false }
    }

    /// This body wrapped in `Tracked`, so the execution's `on_stream_end` callbacks learn whether
    /// it was written to its end. A body of known length is written whole, and one already
    /// wrapped reports its own end: each is returned as it is.
    pub(crate) fn tracked(self, exec: ExecutionRef) -> HttpBody {
        if self.tracked || self.size_hint().exact().is_some() {
            return self;
        }
        let frames = Tracked::new(http_body_util::BodyStream::new(self), exec);
        HttpBody::new(http_body_util::StreamBody::new(frames)).marked()
    }

    /// Whether this body is a stream whose end `on_stream_end` learns: one of unknown length, or
    /// one a `Tracked` inside already reports.
    pub(crate) fn streams(&self) -> bool {
        self.tracked || self.size_hint().exact().is_none()
    }

    /// This body marked as one a `Tracked` inside reports the end of.
    fn marked(mut self) -> HttpBody {
        self.tracked = true;
        self
    }

    /// `wrap(self)` as a body, keeping this body's mark: for a wrapper that passes every frame on.
    pub(crate) fn rewrapped<B>(self, wrap: impl FnOnce(HttpBody) -> B) -> HttpBody
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let tracked = self.tracked;
        let body = HttpBody::new(wrap(self));
        if tracked { body.marked() } else { body }
    }
}

impl Default for HttpBody {
    fn default() -> Self {
        HttpBody::empty()
    }
}

impl From<Bytes> for HttpBody {
    fn from(bytes: Bytes) -> Self {
        HttpBody::from_bytes(bytes)
    }
}

impl From<String> for HttpBody {
    fn from(text: String) -> Self {
        HttpBody::from_bytes(text)
    }
}

impl From<&'static str> for HttpBody {
    fn from(text: &'static str) -> Self {
        HttpBody::from_bytes(text)
    }
}

impl From<Vec<u8>> for HttpBody {
    fn from(bytes: Vec<u8>) -> Self {
        HttpBody::from_bytes(bytes)
    }
}

impl http_body::Body for HttpBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
