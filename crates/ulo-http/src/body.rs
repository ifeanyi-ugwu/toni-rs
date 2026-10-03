use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use http_body::{Frame, SizeHint};
use http_body_util::combinators::UnsyncBoxBody;
use ulo::BoxError;

/// The one body type of requests and responses: a stream of `Bytes` frames, the `http_body::Body`
/// every backend converts to and from at its edge.
///
/// Dropping a request body unread is not a disconnect: `CancelReason::Disconnected` fires only when
/// the backend observes the peer close the connection or reset the stream.
pub struct HttpBody {
    inner: UnsyncBoxBody<Bytes, BoxError>,
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

    /// A streaming body: chunked encoding on HTTP/1.1, data frames on HTTP/2. As a reply it is
    /// wrapped in `ulo_transport::Tracked`, so `on_stream_end` callbacks learn how it ended.
    pub fn stream<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Into<BoxError>,
    {
        let _ = stream;
        todo!("`http_body_util::StreamBody` over the stream mapped to data frames")
    }

    /// Any `http_body::Body` of `Bytes`, its error boxed.
    pub fn new<B>(body: B) -> Self
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        use http_body_util::BodyExt;
        HttpBody { inner: body.map_err(Into::into).boxed_unsync() }
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
