//! The extraction trait, generic over the context an extractor reads from.

use std::fmt;
use std::future::Future;

use crate::context::{HandlerContext, HttpContext};

use super::{FromRequest, FromRequestParts};

/// Extracts a value from the context handling the current message.
///
/// An extractor names the contexts it is valid for by which impls it carries,
/// so reaching for an HTTP extractor in a WebSocket handler is a trait-bound
/// error rather than something a macro catches by type name.
///
/// # Writing one
///
/// Most extractors don't need this trait directly. Reading request metadata is
/// [`FromRequestParts`] — sync, no context, and it gets a `FromContext` impl for
/// free. Reading the body is [`FromRequest`] plus a one-line [`BodyExtractor`]
/// marker. Implement `FromContext` by hand only for something neither covers,
/// such as an extractor spanning several transports.
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: fmt::Display;

    fn extract(ctx: &mut C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}

/// Take the request for a body extractor, or report that it has already gone.
///
/// The whole body of a body extractor's [`FromContext`] impl — write one like this:
///
/// ```rust,ignore
/// impl FromContext<HttpContext> for MyRawBody {
///     type Error = BodyExtractionError<<Self as FromRequest>::Error>;
///
///     async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
///         extract_body::<Self>(ctx).await
///     }
/// }
/// ```
///
/// A concrete impl, not a blanket over some marker trait: a second blanket
/// would collide with the [`FromRequestParts`] one, since two blankets over `T`
/// overlap whatever their bounds while a blanket and a concrete impl do not.
///
/// A handler may have only one body extractor. The macro that builds handlers
/// rejects a second at compile time when it recognises both types; when it
/// cannot — a custom extractor it has never heard of — the second one to run
/// fails with [`BodyExtractionError::AlreadyRead`].
pub async fn extract_body<T: FromRequest>(
    ctx: &mut HttpContext,
) -> Result<T, BodyExtractionError<<T as FromRequest>::Error>> {
    let Some(req) = ctx.take_request() else {
        // The client's request was fine; the handler asked for the body twice.
        // It renders as an extraction failure like any other, but the fault is
        // the application's, so it goes to the log at error level as well.
        let extractor = std::any::type_name::<T>();
        tracing::error!(
            extractor,
            "request body already read by something earlier in this handler"
        );
        return Err(BodyExtractionError::AlreadyRead { extractor });
    };
    T::from_request(req)
        .await
        .map_err(BodyExtractionError::Extract)
}

/// A body extractor either failed on its own terms, or arrived to find the body
/// already read.
#[derive(Debug)]
pub enum BodyExtractionError<E> {
    /// Something earlier in the handler took the body. It is single-use — it may
    /// be a stream — so there is nothing left to hand over.
    AlreadyRead {
        /// The extractor that came up empty.
        extractor: &'static str,
    },
    /// The extractor read the body and rejected it.
    Extract(E),
}

impl<E: fmt::Display> fmt::Display for BodyExtractionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRead { extractor } => write!(
                f,
                "`{extractor}` found the request body already read by something earlier in this \
                 handler. The body can only be read once — take `Bytes` (or `HttpRequest`) and \
                 parse it yourself if you need more than one view of it."
            ),
            Self::Extract(e) => write!(f, "{e}"),
        }
    }
}

/// Metadata extractors reach the context without touching the body, so any
/// number of them can run before the one extractor that reads it.
impl<T: FromRequestParts> FromContext<HttpContext> for T {
    type Error = <T as FromRequestParts>::Error;

    async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
        T::from_request_parts(ctx.request())
    }
}

// The framework's body extractors. Concrete impls, so they coexist with the
// blanket above rather than colliding with it.
macro_rules! framework_body_extractor {
    ($ty:ty) => {
        impl FromContext<HttpContext> for $ty {
            type Error = BodyExtractionError<<$ty as FromRequest>::Error>;

            async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
                extract_body::<$ty>(ctx).await
            }
        }
    };
}

framework_body_extractor!(super::Bytes);
framework_body_extractor!(super::BodyStream);
framework_body_extractor!(super::Multipart);

impl<T: serde::de::DeserializeOwned + Send> FromContext<HttpContext> for super::Json<T> {
    type Error = BodyExtractionError<<Self as FromRequest>::Error>;

    async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
        extract_body::<Self>(ctx).await
    }
}

impl<T: serde::de::DeserializeOwned + Send> FromContext<HttpContext> for super::Body<T> {
    type Error = BodyExtractionError<<Self as FromRequest>::Error>;

    async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
        extract_body::<Self>(ctx).await
    }
}

impl<E: FromRequest + super::ValidatableExtractor + Send> FromContext<HttpContext>
    for super::Validated<E>
{
    type Error = BodyExtractionError<<Self as FromRequest>::Error>;

    async fn extract(ctx: &mut HttpContext) -> Result<Self, Self::Error> {
        extract_body::<Self>(ctx).await
    }
}
