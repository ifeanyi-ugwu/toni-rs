//! The gRPC extractors, each consuming the request body, so a handler taking two of them fails the
//! pairwise check naming both (transports DESIGN §6.1). `#[method(Marker)]` checks the message type
//! each names against the marker's `Request`, and `Streaming<T>` against its shape, at compile time.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tonic::Code;
use tonic::codec::BufferSettings;
use tonic_prost::ProstDecoder;
use ulo_transport::{ExtractError, FromCall};

use crate::transport::{Grpc, GrpcCx, GrpcMetadata};

/// The largest message a request may carry, tonic's own default: a larger one fails extraction as
/// `ExtractError::TooLarge`.
pub(crate) const MESSAGE_LIMIT: usize = 4 * 1024 * 1024;

/// The request message: `Message<pb::GetUserRequest>`.
#[derive(Clone, Debug)]
pub struct Message<T>(pub T);

impl<T> Message<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Message<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        first_message(cx, "message").await.map(Message)
    }
}

/// The whole request: the message with its metadata.
#[derive(Debug)]
pub struct Request<T> {
    pub(crate) message: T,
    pub(crate) metadata: GrpcMetadata,
}

impl<T> Request<T> {
    pub fn message(&self) -> &T {
        &self.message
    }

    pub fn metadata(&self) -> &GrpcMetadata {
        &self.metadata
    }

    pub fn into_inner(self) -> T {
        self.message
    }
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Request<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        let message = first_message(cx, "request").await?;
        Ok(Request { message, metadata: cx.metadata().clone() })
    }
}

/// A client-streamed request's messages, each decoded as `T`; an item that does not decode yields
/// `Err` and ends the stream.
pub struct Streaming<T> {
    pub(crate) inner: tonic::Streaming<T>,
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Streaming<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        let body = cx.take_body().ok_or(ExtractError::Missing { param: "stream" })?;
        let inner = tonic::Streaming::new_request(decoder::<T>(), body, None, Some(MESSAGE_LIMIT));
        Ok(Streaming { inner })
    }
}

impl<T> Stream for Streaming<T> {
    type Item = Result<T, tonic::Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl FromCall<Grpc> for GrpcMetadata {
    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        Ok(cx.metadata().clone())
    }
}

fn decoder<T: prost::Message + Default>() -> ProstDecoder<T> {
    ProstDecoder::new(BufferSettings::default())
}

/// The first message of the request body. A body another extractor took, or one ending before a
/// whole message, is `Missing`; one over [`MESSAGE_LIMIT`] is `TooLarge`; one that does not decode
/// as `T` is `Malformed`, the decoder's status redacted through the app.
async fn first_message<T>(cx: &GrpcCx, param: &'static str) -> Result<T, ExtractError>
where
    T: prost::Message + Default + Send + 'static,
{
    let body = cx.take_body().ok_or(ExtractError::Missing { param })?;
    let mut messages = tonic::Streaming::new_request(decoder::<T>(), body, None, Some(MESSAGE_LIMIT));
    match messages.message().await {
        Ok(Some(message)) => Ok(message),
        Ok(None) => Err(ExtractError::Missing { param }),
        Err(status) if status.code() == Code::OutOfRange => Err(ExtractError::TooLarge { param, limit: MESSAGE_LIMIT as u64 }),
        Err(status) => Err(ExtractError::Malformed { param, source: cx.app().redact(Box::new(status)) }),
    }
}
