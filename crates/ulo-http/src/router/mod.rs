//! The router every backend shares (transports DESIGN §3.2). Built in `prepare`; a duplicate
//! route, or two patterns that differ only in a parameter's name at one position, is a
//! `StartupError::Configure` naming both handlers.
//!
//! A path that matches nothing answers 404. A matching path with the wrong method answers 405
//! with `Allow`, which RFC 9110 requires. `HEAD` is answered from the `GET` handler with the body
//! omitted unless a `HEAD` handler exists. `OPTIONS` without a handler answers 204 with `Allow`;
//! a CORS preflight never reaches routing.

pub(crate) mod pattern;

use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Method};
use ulo::MountedHandler;

use crate::__private::{HandlerFn, HttpHandler};
use crate::cx::PathParams;
use crate::pre_dispatch::{ScopedStage, Stage};
use crate::router::pattern::Pattern;
use crate::transport::Http;

/// Every route, by pattern, each with its methods.
pub(crate) struct Router {
    pub(crate) routes: Vec<RouteEntry>,
}

/// One pattern and the handlers answering it, by method.
pub(crate) struct RouteEntry {
    pub(crate) pattern: Pattern,
    pub(crate) methods: Vec<(Method, Arc<RouteTarget>)>,
}

/// One matched handler, as `prepare` built it.
pub(crate) struct RouteTarget {
    pub(crate) handler: MountedHandler<Http>,
    pub(crate) call: HandlerFn,
    /// The pattern as written, prefix applied, for `HttpCx::route` and the span's `http.route`.
    pub(crate) pattern: Arc<str>,
    pub(crate) body_limit: u64,
    /// The route's `#[meta(Timeout(..))]`.
    pub(crate) timeout: Option<Duration>,
    /// The pre-dispatch entries whose scope covers this route, middleware and tower layers in
    /// declaration order, composed once here; routes with the same set share one stack.
    pub(crate) stage: Arc<ScopedStage>,
}

/// What routing a request decided.
pub(crate) enum Routed<'r> {
    Found {
        target: &'r Arc<RouteTarget>,
        params: PathParams,
        /// A `HEAD` request answered by the `GET` handler: the body is dropped when written.
        head_from_get: bool,
    },
    NotFound,
    /// A path that matches with no handler for the method: 405 with this `Allow`.
    MethodNotAllowed { allow: HeaderValue },
    /// `OPTIONS` on a matching path with no `OPTIONS` handler: 204 with this `Allow`.
    Options { allow: HeaderValue },
}

/// A route-table failure, naming every handler involved.
#[derive(Debug)]
pub(crate) struct RouteError {
    pub(crate) message: String,
}

impl Router {
    /// The table for `handlers`, each `HttpHandler`'s pattern joined to its controller's prefix,
    /// with each `Path<T>` check run against its route and each route's scoped pre-dispatch stage
    /// taken from `stage`. Every failure is returned, not the first.
    pub(crate) fn build(handlers: &[MountedHandler<Http>], default_body_limit: u64, stage: &Stage) -> Result<Router, Vec<RouteError>> {
        let _ = (handlers, default_body_limit, stage, HttpHandler::method);
        todo!("parse and join each pattern, group by pattern, refuse conflicts naming both handlers, run path checks, read `BodyLimit` and `Timeout` from each handler's metadata, `stage.scoped_for(&pattern)`")
    }

    pub(crate) fn route(&self, method: &Method, path: &str) -> Routed<'_> {
        let _ = (method, path);
        todo!("first matching pattern; method, then HEAD-from-GET, then OPTIONS, then 405 with `Allow`")
    }
}
