//! Error handling types and traits for Toni.
//!
//! Handlers return `Result<T, E>` where `E` implements [`Error`] (or
//! [`HttpError`], the convenience type). The framework lifts the error into
//! the active transport's error type and renders the canonical envelope.

pub mod error;
pub mod framework;
pub mod http_error;

pub use error::{Error, ErrorKind};
pub use framework::{
    Cancelled, GuardRejection, MiddlewareFailure, PanicRecovered, PipelineSegment, Unrouted,
};
pub use http_error::{HttpError, http_reason, http_status};
