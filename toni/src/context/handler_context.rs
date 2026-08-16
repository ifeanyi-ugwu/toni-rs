use std::time::{Duration, Instant};

use crate::http_helpers::RouteMetadata;

use super::{CancellationToken, Extensions};

/// The universal interface every per-request context implements.
///
/// Each transport (HTTP, RPC, gRPC, WebSocket) has its own concrete context
/// type with transport-specific fields; they all implement this trait so that
/// **universal** enhancers (guards, interceptors, pipes, error handlers) can
/// be written once via a blanket impl over `C: HandlerContext`.
///
/// Methods on this trait are deliberately limited to what every transport can
/// implement honestly — no method requires a transport to fake an answer. If a
/// concept only makes sense for some transports (HTTP headers, gRPC metadata,
/// WS client identity), it lives on the concrete context, not here.
pub trait HandlerContext: Send {
    /// Per-route metadata (`#[set_metadata(...)]` attached at the controller
    /// or method level). `None` for global handlers (404, error filters) that
    /// never bind to a specific route.
    fn route_metadata(&self) -> Option<&RouteMetadata>;

    /// Per-message typed key-value bag: the channel from one pipeline stage to
    /// the next. A guard attaches a value, a later enhancer or the handler reads
    /// it, neither coupled to the other's type.
    ///
    /// The bag mutates through `&self` — see [`Extensions`].
    fn extensions(&self) -> &Extensions;

    /// The per-request cancellation token. Resolves when the client
    /// disconnects, the server triggers a per-request abort, or the handler's
    /// deadline expires.
    fn cancellation(&self) -> &CancellationToken;

    /// The absolute deadline by which the request should be answered, if the
    /// transport carries one. gRPC populates this from `grpc-timeout`; HTTP
    /// adapters may populate from a header; transports without a deadline
    /// concept (TCP, UDP, WS) return `None`.
    ///
    /// Default impl returns `None` so transports that don't carry deadlines
    /// don't have to override.
    fn deadline(&self) -> Option<Instant> {
        None
    }

    /// Short-circuit the handler chain: subsequent enhancers and the handler
    /// are skipped.
    ///
    /// This stops the chain without supplying an answer, so a guard aborting
    /// this way rejects the request. An enhancer that has an answer to give
    /// returns it instead — see [`Interceptor`](crate::traits_helpers::Interceptor)
    /// and [`Pipe`](crate::traits_helpers::Pipe).
    fn abort(&mut self);

    fn should_abort(&self) -> bool;
}

impl dyn HandlerContext + '_ {
    /// Time remaining until [`deadline`](HandlerContext::deadline), if one is
    /// set. Returns `Duration::ZERO` if the deadline has already passed.
    pub fn time_remaining(&self) -> Option<Duration> {
        self.deadline()
            .map(|d| d.saturating_duration_since(Instant::now()))
    }
}
