use std::future::Future;
use std::ops::Deref;

use http::HeaderMap;
use ulo_transport::{ExtractError, FromCall};

use crate::cx::HttpCx;
use crate::transport::Http;

/// One typed header from the `headers` crate: `Header<headers::Authorization<Bearer>>`. Absent is
/// `ExtractError::Missing` naming the header, so `Option<Header<H>>` is `None` then; present and
/// unparsable is `Malformed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header<H>(pub H);

impl<H> Deref for Header<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.0
    }
}

impl<H: headers::Header + Send + 'static> FromCall<Http> for Header<H> {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("`H::decode` over the request's values for `H::name()`") }
    }
}

/// Every request header.
impl FromCall<Http> for HeaderMap {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.headers().clone())
    }
}

/// The `Last-Event-ID` a reconnecting SSE client sends: `None` on a first connection. It hands over
/// what the client sent and resumes nothing; the handler decides what it means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastEventId(pub Option<String>);

impl FromCall<Http> for LastEventId {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("the header as UTF-8; not UTF-8 is `Malformed`") }
    }
}

/// The context itself, as a handler parameter.
impl FromCall<Http> for HttpCx {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.clone())
    }
}
