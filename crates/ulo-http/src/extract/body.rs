use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;
use ulo_transport::{ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Validate};

use crate::body::HttpBody;
use crate::cx::HttpCx;
use crate::response::Response;
use crate::transport::Http;

/// A JSON body, and a JSON reply: `application/json`, or any `+json` media type on the way in.
/// Consumes the body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Json<T> {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("check the media type (415), read up to the route's limit (413), decode (`Malformed`, redacted)") }
    }
}

/// 200, `application/json`.
impl<T: Serialize + Send + 'static> IntoReply<Http> for Json<T> {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        todo!("serialize; a serializer failure is the `IntoReplyError`")
    }
}

impl<T: Validate> Validate for Json<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// An `application/x-www-form-urlencoded` body. Consumes the body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Form<T>(pub T);

impl<T> Deref for Form<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Form<T> {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("check the media type (415), read up to the limit (413), `serde_urlencoded`") }
    }
}

impl<T: Validate> Validate for Form<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The whole body, up to the route's limit. Consumes the body.
impl FromCall<Http> for Bytes {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("collect the body up to the limit (413)") }
    }
}

/// The body as a stream of chunks, read as it arrives; a chunk past the route's limit ends the
/// stream with an error. Consumes the body.
pub struct BodyStream {
    pub(crate) body: HttpBody,
    pub(crate) limit: u64,
    pub(crate) read: u64,
}

impl Stream for BodyStream {
    type Item = Result<Bytes, BoxError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let _ = (self, cx);
        todo!("poll data frames, counting against the limit")
    }
}

impl FromCall<Http> for BodyStream {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("take the body; `Missing` when already taken") }
    }
}

/// A `multipart/form-data` body, its parts read one at a time. Consumes the body.
pub struct Multipart {
    pub(crate) inner: multer::Multipart<'static>,
}

impl Multipart {
    /// The next part, `None` after the last.
    pub async fn next_field(&mut self) -> Result<Option<multer::Field<'static>>, multer::Error> {
        self.inner.next_field().await
    }
}

impl FromCall<Http> for Multipart {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("read the boundary from `content-type` (415 without one); stream the body under the limit") }
    }
}
