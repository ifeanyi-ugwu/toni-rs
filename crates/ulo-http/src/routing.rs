use std::fmt;
use std::sync::Arc;

use ulo::HandlerInfo;

/// How the application answered a request, in the extensions of every response it gives, on a
/// backend and embedded alike. Host middleware reads it to record the app's route rather than the
/// raw path. Its presence means the app answered: a host serving the app nested or as its fallback
/// finds none on a response it routed elsewhere.
///
/// ```ignore
/// match response.extensions().get::<ulo_http::Routing>() {
///     Some(Routing::Matched { route, .. } | Routing::Options { route }) => metrics.record(route),
///     Some(Routing::NotFound | Routing::MethodNotAllowed) => metrics.record("<miss>"),
///     Some(_) => metrics.record("<unrouted>"),
///     None => {} // not the app's answer
/// }
/// ```
#[non_exhaustive]
#[derive(Clone)]
pub enum Routing {
    /// A handler answered. `route` is its pattern as the host sees it, the embedding's
    /// `.nested_at` prefix and the controller's prefix applied: `/api/users/{id}`.
    Matched { route: Arc<str>, handler: Arc<HandlerInfo> },
    /// An `OPTIONS` on a path whose route has no `OPTIONS` handler, answered by the router with
    /// 204 and `Allow`. `route` is the matched pattern as in `Matched`.
    Options { route: Arc<str> },
    /// No route matches the path.
    NotFound,
    /// A route matches the path and has no handler for the method: a 405 with `Allow`.
    MethodNotAllowed,
    /// The app answered without routing the request: a load-shedding or drain refusal, the
    /// embedding's refusal before `listen()` or after `close`, an unscoped pre-dispatch entry
    /// answering without `next` or failing, and an upgrade request an upgrade handler took.
    Unrouted,
}

impl fmt::Debug for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Routing::Matched { route, handler } => f
                .debug_struct("Matched")
                .field("route", route)
                .field("handler", &format_args!("{}::{}", handler.controller(), handler.name()))
                .finish(),
            Routing::Options { route } => f.debug_struct("Options").field("route", route).finish(),
            Routing::NotFound => f.write_str("NotFound"),
            Routing::MethodNotAllowed => f.write_str("MethodNotAllowed"),
            Routing::Unrouted => f.write_str("Unrouted"),
        }
    }
}
