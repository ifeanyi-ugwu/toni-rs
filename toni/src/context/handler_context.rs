use std::time::{Duration, Instant};

use crate::context::Metadata;

use super::{CancellationToken, Extensions};
use crate::traits_helpers::ExecutionCache;

/// The universal interface every per-request context implements.
///
/// Each transport (HTTP, RPC, gRPC, WebSocket) has its own concrete context
/// type with transport-specific fields; they all implement this trait so that
/// **universal** enhancers (guards, interceptors, error handlers) can
/// be written once via a blanket impl over `C: HandlerContext`.
///
/// Methods on this trait are deliberately limited to what every transport can
/// implement honestly — no method requires a transport to fake an answer. If a
/// concept only makes sense for some transports (HTTP headers, gRPC metadata,
/// WS client identity), it lives on the concrete context, not here.
pub trait HandlerContext: Send + Sync {
    /// What the handler declared about itself with `#[set_metadata(...)]`, on the impl block or on
    /// the handler, with the handler winning where both name one type.
    ///
    /// Distinct from the wire fields a call arrived with, which are `headers()` on the transports
    /// that have them. `None` for global handlers (404, error filters) that never bind to one
    /// handler; a type nothing declared reads back as absent rather than as an error.
    fn metadata(&self) -> Option<&Metadata>;

    /// Per-message typed key-value bag: the channel from one pipeline stage to
    /// the next. A guard attaches a value, a later enhancer or the handler reads
    /// it, neither coupled to the other's type.
    ///
    /// The bag mutates through `&self` — see [`Extensions`].
    fn extensions(&self) -> &Extensions;

    /// The instances built for this execution.
    ///
    /// A request-scoped type resolved twice in one execution is constructed
    /// once and shared. Nothing in here is transport-specific — it lives on the
    /// context because that is the object whose lifetime it shares.
    fn cache(&self) -> &ExecutionCache;

    /// The per-request cancellation token. Resolves when the client
    /// disconnects, the server triggers a per-request abort, or the handler's
    /// deadline expires.
    fn cancellation(&self) -> &CancellationToken;

    /// The absolute deadline by which the request should be answered, if the
    /// transport carries one.
    ///
    /// No transport overrides this yet, so it is `None` everywhere. gRPC's
    /// `grpc-timeout` is the obvious first producer.
    fn deadline(&self) -> Option<Instant> {
        None
    }
}

impl dyn HandlerContext + '_ {
    /// Time remaining until [`deadline`](HandlerContext::deadline), if one is
    /// set. Returns `Duration::ZERO` if the deadline has already passed.
    pub fn time_remaining(&self) -> Option<Duration> {
        self.deadline()
            .map(|d| d.saturating_duration_since(Instant::now()))
    }
}
