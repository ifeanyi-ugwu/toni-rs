//! Middleware: what a pre-dispatch entry runs (transports DESIGN §3.3).
//!
//! `ulo_http::middleware::Next` holds the rest of the pre-dispatch chain; the core's
//! `ulo::Next<'a, T>` holds the interceptor chain. A module importing both qualifies one or writes
//! `use .. as ..`; no alias ships, and the two differ in signature, so a wrong import fails at the
//! type.

use std::future::Future;

use ulo::BoxFuture;

use crate::request::Request;
use crate::response::Response;

/// A pre-dispatch entry's behaviour: sees the request before the handler's guards, may rewrite it,
/// may answer without calling `next`, which is how CORS preflight and authentication work, and
/// may change the response on the way out.
///
/// A middleware type is an ordinary container binding when declared with
/// `PreDispatch::apply::<M>()`, resolved per request through the module that declared the entry,
/// inside the request's execution; a per-execution middleware is built once per request.
/// `apply_value` shares one value.
///
/// It runs inside the error chain: a panic reaches the error handlers as `PanicRecovered`, the
/// matched handler's tiers when the entry is scoped, the global ones when it is not.
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, req: Request, next: Next<'_>) -> impl Future<Output = Response> + Send;
}

/// The rest of the pre-dispatch chain and, after it, routing or dispatch. [`run`](Next::run)
/// consumes it, so the rest runs at most once; a middleware that returns without calling it
/// answers the request itself.
pub struct Next<'a> {
    pub(crate) rest: Box<dyn FnOnce(Request) -> BoxFuture<'a, Response> + Send + 'a>,
}

impl<'a> Next<'a> {
    pub fn run(self, req: Request) -> BoxFuture<'a, Response> {
        (self.rest)(req)
    }

    /// The bound fixes the closure's return type at `BoxFuture<'a, _>`, so a closure answering a
    /// `'static` future coerces into a `Next` borrowing whatever the middleware's `&self` borrows.
    pub(crate) fn new(rest: impl FnOnce(Request) -> BoxFuture<'a, Response> + Send + 'a) -> Self {
        Next { rest: Box::new(rest) }
    }
}

/// The dyn-compatible twin of [`Middleware`] the stage stores.
pub(crate) trait ErasedMiddleware: Send + Sync + 'static {
    fn handle<'a>(&'a self, req: Request, next: Next<'a>) -> BoxFuture<'a, Response>;
}

impl<M: Middleware> ErasedMiddleware for M {
    fn handle<'a>(&'a self, req: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
        Box::pin(Middleware::handle(self, req, next))
    }
}
