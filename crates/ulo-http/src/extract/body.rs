use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use http::StatusCode;
use http_body::Body as _;
use http_body_util::BodyExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;
use ulo_transport::{ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Validate};

use crate::body::HttpBody;
use crate::cx::HttpCx;
use crate::extract::de::{Source, from_pairs, parse_form};
use crate::response::Response;
use crate::transport::Http;

const BODY: &str = "body";

/// A JSON body, and a JSON reply: `application/json`, or any `+json` media type on the way in.
/// Consumes the body.
///
/// An empty body is `ExtractError::Missing`, so `Option<Json<T>>` is `None` for a request carrying
/// none. Another media type is `UnsupportedMediaType` (415), a body over the route's limit
/// `TooLarge` (413) before any of it is decoded, and a body that does not decode `Malformed`.
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
        async move {
            let body = take_present(cx)?;
            if !media_type(cx).is_some_and(is_json) {
                return Err(ExtractError::UnsupportedMediaType { param: BODY, expected: "application/json" });
            }
            let bytes = collect(cx, body).await?;
            serde_json::from_slice(&bytes).map(Json).map_err(|error| ExtractError::Malformed { param: BODY, source: cx.app().redact(Box::new(error)) })
        }
    }
}

/// 200, `application/json`.
impl<T: Serialize + Send + 'static> IntoReply<Http> for Json<T> {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        let bytes = serde_json::to_vec(&self.0).map_err(IntoReplyError::new)?;
        let mut response = Response::new(HttpBody::from_bytes(bytes));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(response)
    }
}

impl<T: Validate> Validate for Json<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// An `application/x-www-form-urlencoded` body. Consumes the body.
///
/// An empty body is `ExtractError::Missing`; another media type is `UnsupportedMediaType` (415);
/// a body over the route's limit is `TooLarge` (413). A required field the form lacks is `Missing`
/// naming it, a value that does not parse `Malformed` naming it.
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
        async move {
            let body = take_present(cx)?;
            if media_type(cx).as_deref() != Some("application/x-www-form-urlencoded") {
                return Err(ExtractError::UnsupportedMediaType { param: BODY, expected: "application/x-www-form-urlencoded" });
            }
            let bytes = collect(cx, body).await?;
            let decoded = parse_form(&bytes);
            let pairs: Vec<(&str, &str)> = decoded.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            from_pairs::<T>(&pairs, Source::Form).map(Form).map_err(|error| error.into_extract(BODY, cx))
        }
    }
}

impl<T: Validate> Validate for Form<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The whole body, up to the route's limit. Consumes the body. A request without a body extracts
/// as empty `Bytes`.
impl FromCall<Http> for Bytes {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = cx.take_body().ok_or(ExtractError::Missing { param: BODY })?;
            collect(cx, body).await
        }
    }
}

/// The body as a stream of chunks, read as it arrives; a chunk past the route's limit ends the
/// stream with an error. Consumes the body.
///
/// The error that ends it at the limit is an `ExtractError::TooLarge`, boxed, and the stream
/// returns `None` after it.
pub struct BodyStream {
    pub(crate) body: HttpBody,
    pub(crate) limit: u64,
    pub(crate) read: u64,
}

impl Stream for BodyStream {
    type Item = Result<Bytes, BoxError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.read > this.limit {
            return Poll::Ready(None);
        }
        loop {
            let Some(frame) = ready!(Pin::new(&mut this.body).poll_frame(cx)) else {
                return Poll::Ready(None);
            };
            let data = match frame {
                Ok(frame) => match frame.into_data() {
                    Ok(data) => data,
                    Err(_trailers) => continue,
                },
                Err(error) => return Poll::Ready(Some(Err(error))),
            };
            this.read = this.read.saturating_add(data.len() as u64);
            if this.read > this.limit {
                return Poll::Ready(Some(Err(BoxError::from(ExtractError::TooLarge { param: BODY, limit: this.limit }))));
            }
            return Poll::Ready(Some(Ok(data)));
        }
    }
}

impl FromCall<Http> for BodyStream {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = cx.take_body().ok_or(ExtractError::Missing { param: BODY })?;
            let limit = body_limit(cx);
            refuse_declared_excess(cx, limit)?;
            Ok(BodyStream { body, limit, read: 0 })
        }
    }
}

/// A `multipart/form-data` body, its parts read one at a time. Consumes the body.
///
/// An empty body is `ExtractError::Missing`; a body that is not `multipart/form-data` with a
/// boundary is `UnsupportedMediaType` (415); a declared length over the route's limit is
/// `TooLarge` (413). A body that turns out longer than the limit while its parts are read fails
/// that `next_field` call.
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
        async move {
            let body = take_present(cx)?;
            let boundary = cx
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| multer::parse_boundary(value).ok())
                .ok_or(ExtractError::UnsupportedMediaType { param: BODY, expected: "multipart/form-data" })?;
            let limit = body_limit(cx);
            refuse_declared_excess(cx, limit)?;
            Ok(Multipart { inner: multer::Multipart::new(BodyStream { body, limit, read: 0 }, boundary) })
        }
    }
}

/// The route's body limit; the server's for a request that matched no route.
fn body_limit(cx: &HttpCx) -> u64 {
    cx.matched().map_or(cx.inner.config.body_limit, |route| route.body_limit)
}

/// The body for an extractor that requires one: `Missing` when it was already taken, and when
/// the request carries none, so an `Option` of the extractor is `None` then. A request carries
/// none when its body has ended before it was read or it declares `Content-Length: 0`.
fn take_present(cx: &HttpCx) -> Result<HttpBody, ExtractError> {
    let body = cx.take_body().ok_or(ExtractError::Missing { param: BODY })?;
    if body.is_end_stream() || declared_length(cx) == Some(0) {
        return Err(ExtractError::Missing { param: BODY });
    }
    Ok(body)
}

/// The essence of `Content-Type`, the type and subtype lowercased without parameters.
fn media_type(cx: &HttpCx) -> Option<String> {
    let value = cx.headers().get(CONTENT_TYPE)?.to_str().ok()?;
    let essence = value.split(';').next().unwrap_or("").trim();
    Some(essence.to_ascii_lowercase())
}

/// `application/json`, or a structured syntax suffix `+json` (RFC 6839) on any type.
fn is_json(essence: String) -> bool {
    essence == "application/json" || essence.split_once('/').is_some_and(|(_, subtype)| subtype.ends_with("+json"))
}

fn declared_length(cx: &HttpCx) -> Option<u64> {
    cx.headers().get(CONTENT_LENGTH)?.to_str().ok()?.trim().parse().ok()
}

/// 413 from `Content-Length` alone, before any of the body is read.
fn refuse_declared_excess(cx: &HttpCx, limit: u64) -> Result<(), ExtractError> {
    match declared_length(cx) {
        Some(length) if length > limit => Err(ExtractError::TooLarge { param: BODY, limit }),
        _ => Ok(()),
    }
}

/// The whole body, counted against the route's limit as it arrives: `TooLarge` the moment it
/// passes, whatever `Content-Length` claimed. A failed read is `Malformed`.
async fn collect(cx: &HttpCx, mut body: HttpBody) -> Result<Bytes, ExtractError> {
    let limit = body_limit(cx);
    refuse_declared_excess(cx, limit)?;
    let mut buffer = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| ExtractError::Malformed { param: BODY, source: cx.app().redact(error) })?;
        if let Ok(data) = frame.into_data() {
            if (buffer.len() as u64).saturating_add(data.len() as u64) > limit {
                return Err(ExtractError::TooLarge { param: BODY, limit });
            }
            buffer.extend_from_slice(&data);
        }
    }
    Ok(buffer.freeze())
}
