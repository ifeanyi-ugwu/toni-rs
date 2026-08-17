use std::error::Error;
use std::fmt;

use bytes::Bytes;
use futures::Stream;
use http_body_util::BodyExt;
use serde_json::Value;

/// Type-erased response body. Adapters consume this via [`Body::into_box_body`].
///
/// `Send + !Sync` — streams passed to [`Body::stream`] need only be `Send`, so a
/// stream holding non-`Sync` state (an `Rc`, a `RefCell`, a single-use adapter
/// body) flows through without an `Arc<Mutex<...>>` wrapper.
pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, Box<dyn Error + Send + Sync>>;

enum BodyInner {
    Buffered(Bytes),
    Streaming(BoxBody),
}

/// Delegates to an inner body while holding something alive alongside it.
///
/// `UnsyncBoxBody` is a `Pin<Box<_>>` and therefore `Unpin`, so the projection
/// needs no pin machinery.
struct ScopedBody {
    inner: BoxBody,
    _keep_alive: Box<dyn std::any::Any + Send>,
}

impl http_body::Body for ScopedBody {
    type Data = Bytes;
    type Error = Box<dyn Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

impl fmt::Debug for BodyInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BodyInner::Buffered(b) => write!(f, "Buffered({} bytes)", b.len()),
            BodyInner::Streaming(_) => write!(f, "Streaming(...)"),
        }
    }
}

/// An HTTP response body.
///
/// Use the static constructors for buffered content, or [`Body::stream`] for
/// large or generated responses that should not be fully loaded into memory.
///
/// # Example
///
/// ```rust,ignore
/// // Buffered
/// Body::text("hello")
/// Body::json(json!({"ok": true}))
///
/// // Streaming
/// use futures::stream;
/// use bytes::Bytes;
///
/// Body::stream(stream::iter(vec![
///     Ok::<Bytes, std::io::Error>(Bytes::from("chunk 1 ")),
///     Ok(Bytes::from("chunk 2")),
/// ]))
/// .with_content_type("text/plain; charset=utf-8")
/// ```
#[derive(Debug)]
pub struct Body {
    inner: BodyInner,
    content_type: Option<String>,
    /// Held for exactly as long as the body is.
    ///
    /// An execution is not over when the handler returns — it is over when the
    /// answer is. Whatever the dispatcher parks here is dropped by the adapter
    /// after the last frame, not at handler return.
    keep_alive: Option<Box<dyn std::any::Any + Send>>,
}

impl Body {
    /// Plain text body. Sets `Content-Type: text/plain; charset=utf-8`.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            inner: BodyInner::Buffered(Bytes::from(s.into().into_bytes())),
            content_type: Some("text/plain; charset=utf-8".to_string()),
            keep_alive: None,
        }
    }

    /// JSON body from a [`serde_json::Value`]. Sets `Content-Type: application/json`.
    pub fn json(value: Value) -> Self {
        Self {
            inner: BodyInner::Buffered(Bytes::from(serde_json::to_vec(&value).unwrap_or_default())),
            content_type: Some("application/json".to_string()),
            keep_alive: None,
        }
    }

    /// Raw binary body. Sets `Content-Type: application/octet-stream`.
    pub fn binary(data: impl Into<Vec<u8>>) -> Self {
        Self {
            inner: BodyInner::Buffered(Bytes::from(data.into())),
            content_type: Some("application/octet-stream".to_string()),
            keep_alive: None,
        }
    }

    /// Empty body with no content-type.
    pub fn empty() -> Self {
        Self {
            inner: BodyInner::Buffered(Bytes::new()),
            content_type: None,
            keep_alive: None,
        }
    }

    /// Streaming body. Chunks produced by `stream` are forwarded to the adapter
    /// without buffering.
    ///
    /// Content-type is not set automatically — call `.with_content_type()` or
    /// include a `Content-Type` header on the response.
    pub fn stream<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Into<Box<dyn Error + Send + Sync>> + 'static,
    {
        use futures::StreamExt;
        use http_body_util::StreamBody;

        let frames = stream.map(|r| r.map(http_body::Frame::data).map_err(Into::into));
        Self {
            inner: BodyInner::Streaming(BodyExt::boxed_unsync(StreamBody::new(frames))),
            content_type: None,
            keep_alive: None,
        }
    }

    /// Override or set the content-type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// The content-type this body carries, if any.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The raw bytes of a buffered body. Returns `None` for streaming bodies.
    pub fn try_bytes(&self) -> Option<&Bytes> {
        match &self.inner {
            BodyInner::Buffered(bytes) => Some(bytes),
            BodyInner::Streaming(_) => None,
        }
    }

    /// Whether this is a streaming body.
    pub fn is_streaming(&self) -> bool {
        matches!(self.inner, BodyInner::Streaming(_))
    }

    /// Wrap an already-erased body, preserving its streaming nature.
    ///
    /// The content-type is not set automatically — call `.with_content_type()` if needed.
    pub fn from_box_body(box_body: BoxBody) -> Self {
        Self {
            inner: BodyInner::Streaming(box_body),
            content_type: None,
            keep_alive: None,
        }
    }

    /// Consume this body and return a [`BoxBody`] for the adapter to write.
    /// Keep `value` alive until this body is dropped.
    ///
    /// The dispatcher parks the execution context here so a streaming answer can
    /// still reach the bag it was built with. Buffered bodies keep reporting as
    /// buffered — the guard rides alongside rather than changing what this is.
    pub fn keep_alive<T: std::any::Any + Send>(mut self, value: T) -> Self {
        self.keep_alive = Some(Box::new(value));
        self
    }

    pub fn into_box_body(self) -> BoxBody {
        let Self {
            inner, keep_alive, ..
        } = self;
        let body = match inner {
            BodyInner::Buffered(bytes) => http_body_util::Full::new(bytes)
                .map_err(|never: std::convert::Infallible| match never {})
                .boxed_unsync(),
            BodyInner::Streaming(box_body) => box_body,
        };
        match keep_alive {
            Some(guard) => ScopedBody {
                inner: body,
                _keep_alive: guard,
            }
            .boxed_unsync(),
            None => body,
        }
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self {
            inner: BodyInner::Buffered(bytes),
            content_type: None,
            keep_alive: None,
        }
    }
}
