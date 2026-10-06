//! The RPC extractors: [`Payload<T>`] and [`Inbound<T>`], each consuming the call's payload, a bare
//! `Bytes` read as `Payload<Bytes>` is, and `CallHeaders` (in `transport`).

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use futures_core::stream::BoxStream;
use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FromCall};

use crate::frame::Data;
use crate::transport::{Body, Rpc, RpcCx};

/// The name an extraction failure gives the payload.
const PAYLOAD: &str = "payload";

/// The call's payload, `d`, decoded by the link's codec: `Payload<NewInvoice>`. `Payload<Bytes>`
/// is raw bytes, refused in `prepare` on a link declaring `binary: false`. A payload that does not
/// decode fails as `Malformed`, answered `bad_request`.
#[derive(Clone, Debug)]
pub struct Payload<T>(pub T);

impl<T> Payload<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Rpc> for Payload<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let data = take_data(cx)?;
        decode(cx, &data).map(Payload)
    }
}

/// The payload as raw bytes, the same read as `Payload<Bytes>`.
impl FromCall<Rpc> for Bytes {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let data = take_data(cx)?;
        decode(cx, &data)
    }
}

/// A call's one `d`. A streamed request never reaches a handler reading it: the server answers
/// `open` on such a pattern `bad_request` before dispatch.
fn take_data(cx: &RpcCx) -> Result<Data, ExtractError> {
    match cx.take_body() {
        Some(Body::Data(data)) => Ok(data),
        Some(Body::Stream(_)) | None => Err(ExtractError::Missing { param: PAYLOAD }),
    }
}

fn decode<T: DeserializeOwned>(cx: &RpcCx, data: &Data) -> Result<T, ExtractError> {
    cx.codec().decode(data).map_err(|err| ExtractError::Malformed { param: PAYLOAD, source: cx.app().redact(err) })
}

/// A streamed request's items, `in` frames until `in_end`, each decoded as `T`: a parameter of
/// this type makes the handler's shape a streamed request. An item that does not decode yields
/// `Err` and ends the stream.
pub struct Inbound<T> {
    pub(crate) items: BoxStream<'static, Result<T, ExtractError>>,
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Rpc> for Inbound<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let Some(Body::Stream(receiver)) = cx.take_body() else {
            return Err(ExtractError::Missing { param: PAYLOAD });
        };
        let cx = cx.clone();
        let items = futures_util::stream::unfold(Some(receiver), move |receiver| {
            let cx = cx.clone();
            async move {
                let mut receiver = receiver?;
                let data = receiver.recv().await?;
                match decode::<T>(&cx, &data) {
                    Ok(item) => Some((Ok(item), Some(receiver))),
                    Err(err) => Some((Err(err), None)),
                }
            }
        });
        Ok(Inbound { items: Box::pin(items) })
    }
}

impl<T> Stream for Inbound<T> {
    type Item = Result<T, ExtractError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}
