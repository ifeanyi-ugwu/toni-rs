use std::future::Future;
use std::ops::Deref;

use http::HeaderMap;
use ulo_transport::{ExtractError, FromCall};

use crate::cx::HttpCx;
use crate::transport::Http;

const LAST_EVENT_ID: &str = "last-event-id";

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
        async move {
            let name = H::name();
            let param = name.as_str();
            let mut values = cx.headers().get_all(name).iter().peekable();
            if values.peek().is_none() {
                return Err(ExtractError::Missing { param });
            }
            H::decode(&mut values).map(Header).map_err(|error| ExtractError::Malformed { param, source: cx.app().redact(Box::new(error)) })
        }
    }
}

/// Every request header.
impl FromCall<Http> for HeaderMap {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.headers().clone())
    }
}

/// The `Last-Event-ID` a reconnecting SSE client sends: `None` on a first connection. It hands over
/// what the client sent and resumes nothing; the handler decides what it means. A value that is
/// not UTF-8 is `ExtractError::Malformed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastEventId(pub Option<String>);

impl FromCall<Http> for LastEventId {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let Some(value) = cx.headers().get(LAST_EVENT_ID) else {
                return Ok(LastEventId(None));
            };
            match std::str::from_utf8(value.as_bytes()) {
                Ok(id) => Ok(LastEventId(Some(id.to_owned()))),
                Err(error) => Err(ExtractError::Malformed { param: LAST_EVENT_ID, source: cx.app().redact(Box::new(error)) }),
            }
        }
    }
}

/// The context itself, as a handler parameter.
impl FromCall<Http> for HttpCx {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.clone())
    }
}
