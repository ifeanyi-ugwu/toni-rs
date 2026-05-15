//! Per-request execution contexts.
//!
//! Each transport (HTTP, RPC, WebSocket) has its own concrete context type
//! with transport-specific fields. They all implement [`HandlerContext`], the
//! universal interface that lets a single enhancer (guard / interceptor /
//! pipe / error handler) be written for one transport, all transports, or a
//! chosen subset.

mod cancellation;
mod extensions;
mod grpc;
mod handler_context;
mod http;
mod rpc;
pub(crate) mod shared;
mod ws;

pub use self::cancellation::CancellationToken;
pub use self::extensions::Extensions;
pub use self::grpc::GrpcContext;
pub use self::handler_context::HandlerContext;
pub use self::http::HttpContext;
pub use self::rpc::RpcContext;
pub use self::ws::WsContext;
