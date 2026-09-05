//! The extraction trait, generic over the context an extractor reads from.

use std::fmt;
use std::future::Future;

use crate::context::{HandlerContext, HttpContext};
use crate::http_helpers::HttpRequest;

/// Extracts a value from the context handling the current message.
///
/// An extractor names the contexts it is valid for by which impls it carries,
/// so reaching for an HTTP extractor in a WebSocket handler is a trait-bound
/// error rather than something a macro catches by type name.
///
/// # Writing one
///
/// On HTTP, an extractor that reads metadata borrows the request parts from
/// [`HttpContext::request`], which leaves the body alone; one that reads the
/// body takes it with [`take_body`]. See the [module docs](super) for both
/// shapes.
///
/// [`HttpContext::request`]: crate::context::HttpContext::request
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: fmt::Display;

    /// Whether extracting this consumes what it reads, leaving nothing for a
    /// second extractor.
    ///
    /// The request body on HTTP, where it may be a stream and there is nothing
    /// to hand a second reader. Nothing on RPC or WebSocket, where the payload
    /// is buffered and reads freely.
    ///
    /// The handler macros sum this across a handler's parameters and reject two
    /// consumers at compile time. An extractor that reads the body through
    /// [`take_body`] and leaves this `false` is not counted, and the second one
    /// to run fails at request time instead.
    const CONSUMES: bool = false;

    fn extract(ctx: &C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}

/// `None` where the inner extractor fails, consuming whatever it consumes.
impl<C: HandlerContext, T: FromContext<C>> FromContext<C> for Option<T> {
    type Error = std::convert::Infallible;

    const CONSUMES: bool = T::CONSUMES;

    async fn extract(ctx: &C) -> Result<Self, Self::Error> {
        Ok(T::extract(ctx).await.ok())
    }
}

/// The whole request, body included, under the same single-use rule as any
/// other body reader.
impl FromContext<HttpContext> for HttpRequest {
    type Error = BodyAlreadyRead;

    const CONSUMES: bool = true;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        take_body::<Self>(ctx)
    }
}

/// Take the request for a body extractor, or report that the body has gone.
///
/// The first line of a body extractor's [`FromContext`] impl — `?` lifts the
/// failure into [`BodyExtractionError`]:
///
/// ```rust,ignore
/// impl FromContext<HttpContext> for MyRawBody {
///     type Error = BodyExtractionError<MyError>;
///
///     async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
///         let req = take_body::<Self>(ctx)?;
///         // read `req`, mapping your own failures through
///         // `BodyExtractionError::Extract`
///     }
/// }
/// ```
///
/// A handler may have only one body extractor. An extractor that sets
/// [`FromContext::CONSUMES`] is counted, and a second one is rejected at compile
/// time; one that reads the body without declaring it reaches here instead.
pub fn take_body<T>(ctx: &HttpContext) -> Result<HttpRequest, BodyAlreadyRead> {
    match ctx.take_request() {
        Some(req) => Ok(req),
        None => {
            // The client's request was fine; the handler asked for the body twice.
            // It renders as an extraction failure like any other, but the fault is
            // the application's, so it goes to the log at error level as well.
            let extractor = std::any::type_name::<T>();
            tracing::error!(
                extractor,
                "request body already read by something earlier in this handler"
            );
            Err(BodyAlreadyRead { extractor })
        }
    }
}

/// Something earlier in the handler took the body. It is single-use — it may be
/// a stream — so there is nothing left to hand over.
#[derive(Debug)]
pub struct BodyAlreadyRead {
    /// The extractor that came up empty.
    pub extractor: &'static str,
}

impl fmt::Display for BodyAlreadyRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` found the request body already read by something earlier in this handler. The \
             body can only be read once — take `Bytes` (or `HttpRequest`) and parse it yourself if \
             you need more than one view of it.",
            self.extractor
        )
    }
}

impl std::error::Error for BodyAlreadyRead {}

/// A body extractor either failed on its own terms, or arrived to find the body
/// already read.
#[derive(Debug)]
pub enum BodyExtractionError<E> {
    /// Nothing was left to read.
    AlreadyRead(BodyAlreadyRead),
    /// The extractor read the body and rejected it.
    Extract(E),
}

impl<E> From<BodyAlreadyRead> for BodyExtractionError<E> {
    fn from(taken: BodyAlreadyRead) -> Self {
        Self::AlreadyRead(taken)
    }
}

impl<E: fmt::Display> fmt::Display for BodyExtractionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRead(taken) => write!(f, "{taken}"),
            Self::Extract(e) => write!(f, "{e}"),
        }
    }
}
