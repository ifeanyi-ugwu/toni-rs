//! The RPC extractors: [`Payload<T>`] and [`Inbound<T>`], each consuming the call's payload, and
//! `CallHeaders` (in `transport`).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use futures_core::stream::BoxStream;
use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FromCall};

use crate::transport::{Rpc, RpcCx};

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
        let _ = cx;
        todo!()
    }
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
        let _ = cx;
        todo!()
    }
}

impl<T> Stream for Inbound<T> {
    type Item = Result<T, ExtractError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}
