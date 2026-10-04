//! The gRPC extractors, each consuming the request body, so a handler taking two of them fails the
//! pairwise check naming both (transports DESIGN §6.1).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use ulo_transport::{ExtractError, FromCall};

use crate::transport::{Grpc, GrpcCx, GrpcMetadata};

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
        let _ = cx;
        todo!()
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
        let _ = cx;
        todo!()
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
        let _ = cx;
        todo!()
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
        let _ = cx;
        todo!()
    }
}
