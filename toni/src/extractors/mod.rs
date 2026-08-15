//! Extractors for request data
//!
//! A handler parameter is anything implementing [`FromContext`] for that
//! handler's context, which is what makes an HTTP extractor a compile error in a
//! WebSocket handler rather than something a macro has to catch by name.
//!
//! Two shorthands cover almost everything, and both produce a `FromContext` impl:
//!
//! - [`FromRequestParts`] — sync, metadata only (headers, path params, query
//!   params). Implemented by `Path` and `Query`. Gets `FromContext<HttpContext>`
//!   through a blanket impl, so writing one of these needs nothing else.
//!
//! - [`FromRequest`] — async, takes the whole request. Implemented by `Json`,
//!   `Bytes`, `Body`, `BodyStream`, `Validated` and `Multipart`, each paired with
//!   a small `FromContext` impl that calls [`extract_body`].
//!
//! Only one extractor per handler can read the body, which is single-use because
//! it may be a stream. The handler macro rejects a second one it recognises at
//! compile time; one it doesn't recognise fails at extraction with
//! [`BodyExtractionError::AlreadyRead`], naming itself.

mod body;
mod body_stream;
mod bytes;
mod from_context;
mod json;
pub mod multipart;
mod path;
mod query;
mod validated;

pub use body::Body;
pub use body_stream::BodyStream;
pub use bytes::Bytes;
pub use from_context::{BodyExtractionError, FromContext, extract_body};
pub use json::Json;
pub use multipart::{Multipart, MultipartError};
pub use path::Path;
pub use query::Query;
pub use validated::{ValidatableExtractor, Validated, ValidationError};

use crate::http_helpers::{HttpRequest, RequestPart};

/// Extracts a value from request metadata (method, URI, headers, extensions,
/// path params, query params). Sync and non-consuming — safe to call multiple
/// times per request without touching the body.
pub trait FromRequestParts: Sized {
    type Error: std::fmt::Display;

    fn from_request_parts(parts: &RequestPart) -> Result<Self, Self::Error>;
}

/// Extracts a value from the full request, potentially consuming the body.
/// Async and single-use for body-consuming implementations — only one
/// body-reading extractor may appear per handler.
///
/// All [`FromRequestParts`] types automatically implement this trait via a
/// blanket impl that ignores the body.
pub trait FromRequest: Sized {
    type Error: std::fmt::Display + Send + Sync + 'static;

    fn from_request(
        req: HttpRequest,
    ) -> impl std::future::Future<Output = Result<Self, Self::Error>> + Send;
}

impl<T: FromRequestParts> FromRequest for T
where
    <T as FromRequestParts>::Error: Send + Sync + 'static,
{
    type Error = <T as FromRequestParts>::Error;

    async fn from_request(req: HttpRequest) -> Result<Self, Self::Error> {
        let (parts, _) = req.0.into_parts();
        T::from_request_parts(&parts)
    }
}
