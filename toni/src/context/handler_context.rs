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

    /// Per-request typed key-value bag. Use to attach values from one enhancer
    /// and read them from another without coupling their types.
    fn extensions(&self) -> &Extensions;

    fn extensions_mut(&mut self) -> &mut Extensions;

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

    /// Short-circuit the handler chain. Subsequent enhancers and the handler
    /// itself are skipped; whatever response is currently set on the context
    /// becomes the reply.
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
