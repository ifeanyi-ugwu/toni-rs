//! Extractors for request data
//!
//! A handler parameter is anything implementing [`FromContext`] for that
//! handler's context, which is what makes an HTTP extractor a compile error in a
//! WebSocket handler rather than something a macro has to catch by name.
//!
//! On HTTP there are two shapes, and which one an extractor is written in
//! decides whether it touches the body:
//!
//! - Metadata — headers, path params, query params, extensions. Borrow the
//!   request parts from [`HttpContext::request`], which leaves the body
//!   untouched. `Path` and `Query` are written this way, and any number of them
//!   can run on one handler.
//!
//! - The body. Take it with [`take_body`], which yields it once and names the
//!   second asker. `Json`, `Bytes`, `Body`, `BodyStream` and `Multipart` are
//!   written this way.
//!
//! Only one extractor per handler can read the body, which is single-use because
//! it may be a stream. The handler macro rejects a second one it recognises at
//! compile time; one it doesn't recognise fails at extraction with
//! [`BodyExtractionError`], naming itself.
//!
//! [`HttpContext::request`]: crate::context::HttpContext::request

mod body;
mod body_stream;
mod bytes;
mod from_context;
mod json;
pub mod multipart;
mod path;
mod payload;
mod query;
mod validated;

pub use body::Body;
pub use body_stream::BodyStream;
pub use bytes::Bytes;
pub use from_context::{BodyAlreadyRead, BodyExtractionError, FromContext, take_body};
pub use json::Json;
pub use multipart::{Multipart, MultipartError};
pub use path::Path;
pub use payload::Payload;
pub use query::Query;
pub use validated::{ValidatableExtractor, Validated, ValidationError};
