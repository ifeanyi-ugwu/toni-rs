//! Middleware: what a pre-dispatch entry runs (transports DESIGN §3.3).
//!
//! `ulo_http::middleware::Next` holds the rest of the pre-dispatch chain; the core's
//! `ulo::Next<'a, T>` holds the interceptor chain. A module importing both qualifies one or writes
//! `use .. as ..`; no alias ships, and the two differ in signature, so a wrong import fails at the
//! type.

use std::future::Future;

use ulo::{BoxError, BoxFuture};

use crate::request::Request;
use crate::response::Response;

/// A pre-dispatch entry's behaviour: sees the request before the handler's guards, may rewrite it,
/// may answer without calling `next`, which is how CORS preflight works, may fail, which is how
/// authentication works, and may change the response on the way out.
///
/// ```ignore
/// impl Middleware for ApiKeyAuth {
///     async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
///         if !self.admits(req.headers()) {
///             return Err(CallError::unauthorized("ApiKey").into());
///         }
///         next.run(req).await
///     }
/// }
/// ```
///
/// A middleware type is an ordinary container binding when declared with
/// `PreDispatch::apply::<M>()`, resolved per request through the module that declared the entry,
/// inside the request's execution; a per-execution middleware is built once per request.
/// `apply_value` shares one value.
///
/// It runs inside the error chain. An `Err` it returns reaches the error handlers as that error,
/// and a panic as `PanicRecovered`: the matched handler's tiers then the global ones when the
/// entry is scoped, the global ones when it is not. What they answer, or the error rendered when
/// none claims it, goes back to the entries before this one as their `next.run`'s `Ok`.
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, req: Request, next: Next<'_>) -> impl Future<Output = Result<Response, BoxError>> + Send;
}

/// The rest of the pre-dispatch chain and, after it, routing or dispatch. [`run`](Next::run)
/// consumes it, so the rest runs at most once; a middleware that returns without calling it
/// answers the request itself.
pub struct Next<'a> {
    pub(crate) rest: Box<dyn FnOnce(Request) -> BoxFuture<'a, Result<Response, BoxError>> + Send + 'a>,
}

impl<'a> Next<'a> {
    /// The rest's response. It is always `Ok`: a failure further in has reached the error handlers
    /// and been answered or rendered there, so an outer middleware, CORS for example, still
    /// decorates the response it becomes. The `Result` matches [`Middleware::handle`], so
    /// `next.run(req).await` is a middleware's answer as it stands.
    pub fn run(self, req: Request) -> BoxFuture<'a, Result<Response, BoxError>> {
        (self.rest)(req)
    }

    /// The bound fixes the closure's return type at `BoxFuture<'a, _>`, so a closure answering a
    /// `'static` future coerces into a `Next` borrowing whatever the middleware's `&self` borrows.
    pub(crate) fn new(rest: impl FnOnce(Request) -> BoxFuture<'a, Result<Response, BoxError>> + Send + 'a) -> Self {
        Next { rest: Box::new(rest) }
    }
}

/// The dyn-compatible twin of [`Middleware`] the stage stores.
pub(crate) trait ErasedMiddleware: Send + Sync + 'static {
    fn handle<'a>(&'a self, req: Request, next: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>>;
}

impl<M: Middleware> ErasedMiddleware for M {
    fn handle<'a>(&'a self, req: Request, next: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>> {
        Box::pin(Middleware::handle(self, req, next))
    }
}
