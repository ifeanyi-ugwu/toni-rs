//! Error handling types and traits for Ulo.
//!
//! Handlers return `Result<T, E>` where `E` implements [`Error`]. The framework lifts the error
//! into the active transport's error type — [`HttpError`](crate::http::HttpError),
//! [`RpcError`](crate::rpc::RpcError), [`WsError`](crate::ws::WsError),
//! [`GrpcStatus`](crate::grpc::GrpcStatus) — and renders the canonical envelope.
//!
//! The kinds here are what every transport shares: [`ErrorKind`], and the framework events a
//! `#[catch]` handler can claim.

mod error;
mod framework;

pub use error::{Error, ErrorKind};
pub use framework::{
    GuardRejection, HookFailed, MiddlewareFailure, PanicRecovered, PipelineSegment, Unrouted,
};
