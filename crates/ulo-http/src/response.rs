//! Replies (transports DESIGN §3.1): any `T: IntoReply<Http>`. `Json<T>`'s reply impl sits beside
//! its extractor in `extract::body`, and `Sse<S>`'s in `sse`.

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue, LOCATION};
use http::StatusCode;
use ulo_transport::{IntoReply, IntoReplyError};

use crate::body::HttpBody;
use crate::cx::HttpCx;
use crate::transport::Http;

/// A response as the `http` crate builds it: `Response::builder().status(..).header(..).body(..)`
/// for any status, headers and body.
pub type Response = http::Response<HttpBody>;

/// 201 with a `Location` header and a body: `Created::at(format!("/users/{}", id), Json(user))`.
pub struct Created<T> {
    location: String,
    body: T,
}

impl<T> Created<T> {
    pub fn at(location: impl Into<String>, body: T) -> Self {
        Created { location: location.into(), body }
    }
}

/// 204 with no body.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoContent;

/// A reply with headers added: `WithHeaders::new(Json(user)).header("x-cache", "miss")`. A header
/// that does not parse fails the reply as `IntoReplyError` before anything is written.
pub struct WithHeaders<T> {
    reply: T,
    headers: Vec<(String, String)>,
}

impl<T> WithHeaders<T> {
    pub fn new(reply: T) -> Self {
        WithHeaders { reply, headers: Vec::new() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// As built.
impl IntoReply<Http> for Response {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        Ok(self)
    }
}

/// 200 with this body and no `Content-Type`.
impl IntoReply<Http> for HttpBody {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        Ok(Response::new(self))
    }
}

/// 204.
impl IntoReply<Http> for () {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        NoContent.into_reply(cx)
    }
}

impl IntoReply<Http> for NoContent {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        let mut response = Response::new(HttpBody::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        Ok(response)
    }
}

/// 200, `application/octet-stream`.
impl IntoReply<Http> for Bytes {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        Ok(with_content_type(HttpBody::from_bytes(self), "application/octet-stream"))
    }
}

/// 200, `text/plain; charset=utf-8`.
impl IntoReply<Http> for String {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        Ok(with_content_type(HttpBody::from_bytes(self), "text/plain; charset=utf-8"))
    }
}

impl IntoReply<Http> for &'static str {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        self.to_owned().into_reply(cx)
    }
}

/// The body's reply with status 201 and `Location`. A location that is not a valid header value
/// fails the reply before anything is written.
impl<T: IntoReply<Http>> IntoReply<Http> for Created<T> {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let location = HeaderValue::try_from(self.location).map_err(IntoReplyError::new)?;
        let mut response = self.body.into_reply(cx)?;
        *response.status_mut() = StatusCode::CREATED;
        response.headers_mut().insert(LOCATION, location);
        Ok(response)
    }
}

/// `T`'s reply with its status replaced.
impl<T: IntoReply<Http>> IntoReply<Http> for (StatusCode, T) {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let (status, reply) = self;
        let mut response = reply.into_reply(cx)?;
        *response.status_mut() = status;
        Ok(response)
    }
}

impl<T: IntoReply<Http>> IntoReply<Http> for WithHeaders<T> {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let mut response = self.reply.into_reply(cx)?;
        for (name, value) in self.headers {
            let name = HeaderName::try_from(name).map_err(IntoReplyError::new)?;
            let value = HeaderValue::try_from(value).map_err(IntoReplyError::new)?;
            response.headers_mut().append(name, value);
        }
        Ok(response)
    }
}

fn with_content_type(body: HttpBody, content_type: &'static str) -> Response {
    let mut response = Response::new(body);
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
