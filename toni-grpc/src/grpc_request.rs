//! What a handler's first parameter says the call carries.
//!
//! `#[grpc_methods]` writes the proto trait's signature, which names the
//! message type — and a macro reads tokens, so an aliased `P<GreetRequest>`
//! tells it nothing. The signature is written as a projection instead, and the
//! compiler resolves what the macro cannot:
//!
//! ```ignore
//! async fn greet(&self, request: <Payload<GreetRequest> as GrpcRequest>::Request)
//!     -> Result<tonic::Response<GreetReply>, tonic::Status>
//! ```
//!
//! The trait lives here rather than in toni core because its impls name
//! `tonic::Request`, and because a trait declared in core could not be
//! implemented here for a type core owns.

use toni::extractors::{Inbound, Payload};
use toni::{GrpcCode, GrpcStatus};

/// A type a gRPC handler can take its request as.
///
/// Implemented for the three shapes a request arrives in: the message
/// ([`Payload<T>`]), the caller's stream ([`Inbound<T>`]), and the whole
/// `tonic::Request<T>` for a handler that wants the wire's own view.
pub trait GrpcRequest {
    /// What the wire carries — the message, or a stream of them.
    type Arg;

    /// The request the proto trait's method is declared with. Always
    /// `tonic::Request<Self::Arg>`; named as an associated type so the macro
    /// can write a signature without knowing which of these it is writing.
    type Request;

    fn from_request(request: Self::Request) -> Self;
}

impl<T> GrpcRequest for Payload<T> {
    type Arg = T;
    type Request = tonic::Request<T>;

    fn from_request(request: Self::Request) -> Self {
        Payload(request.into_inner())
    }
}

impl<T: Send + 'static> GrpcRequest for Inbound<T> {
    type Arg = tonic::Streaming<T>;
    type Request = tonic::Request<tonic::Streaming<T>>;

    /// The caller's stream fails with tonic's status; a handler reading one
    /// sees toni's, so the conversion happens here rather than in every
    /// handler.
    fn from_request(request: Self::Request) -> Self {
        Inbound::new(toni::futures::StreamExt::map(
            request.into_inner(),
            |item| {
                item.map_err(|status| {
                    GrpcStatus::new(
                        GrpcCode::from_i32(status.code() as i32),
                        status.message().to_string(),
                    )
                })
            },
        ))
    }
}

/// The wire's own view — trailers, the peer address, the metadata map as it
/// arrived.
impl<T> GrpcRequest for tonic::Request<T> {
    type Arg = T;
    type Request = tonic::Request<T>;

    fn from_request(request: Self::Request) -> Self {
        request
    }
}
