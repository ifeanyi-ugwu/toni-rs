//! Per-request execution contexts.
//!
//! Each transport (HTTP, RPC, gRPC, WebSocket) has its own concrete context
//! type with transport-specific fields. They all implement [`HandlerContext`],
//! the universal interface that lets a single enhancer (guard / interceptor /
//! pipe / error handler) be written for one transport, all transports, or a
//! chosen subset.

mod cancellation;
mod extensions;
mod handler_context;

pub use self::cancellation::CancellationToken;
pub use self::extensions::Extensions;
pub use self::handler_context::HandlerContext;
