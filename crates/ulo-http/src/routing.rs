use std::fmt;
use std::sync::Arc;

use ulo::HandlerInfo;

/// How the application routed a request, in the extensions of every response it answers after
/// routing, on a backend and embedded alike. Host middleware reads it to record the app's route
/// rather than the raw path:
///
/// ```ignore
/// match response.extensions().get::<ulo_http::Routing>() {
///     Some(Routing::Matched { route, .. }) => metrics.record(route),
///     Some(Routing::NotFound | Routing::MethodNotAllowed) => metrics.record("<miss>"),
///     None => {} // answered before routing
/// }
/// ```
///
/// Absent from a response the app gave before routing: a load-shedding or drain refusal, the
/// embedding's refusal before `listen()`, an unscoped pre-dispatch entry answering without
/// `next`, and an upgrade request an upgrade handler took.
#[non_exhaustive]
#[derive(Clone)]
pub enum Routing {
    /// A handler answered. `route` is its pattern as the host sees it, the embedding's
    /// `.nested_at` prefix and the controller's prefix applied: `/api/users/{id}`.
    Matched { route: Arc<str>, handler: Arc<HandlerInfo> },
    /// No route matches the path.
    NotFound,
    /// A route matches the path and has no handler for the method: a 405, or the 204 with
    /// `Allow` the router answers an `OPTIONS` with.
    MethodNotAllowed,
}

impl fmt::Debug for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Routing::Matched { route, handler } => f
                .debug_struct("Matched")
                .field("route", route)
                .field("handler", &format_args!("{}::{}", handler.controller(), handler.name()))
                .finish(),
            Routing::NotFound => f.write_str("NotFound"),
            Routing::MethodNotAllowed => f.write_str("MethodNotAllowed"),
        }
    }
}
