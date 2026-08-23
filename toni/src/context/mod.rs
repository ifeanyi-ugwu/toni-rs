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
mod metadata;
mod rpc;
pub(crate) mod shared;
mod standalone;
mod ws;

pub use self::cancellation::CancellationToken;
pub use self::extensions::Extensions;
pub use self::grpc::GrpcContext;
pub use self::handler_context::HandlerContext;
pub use self::http::HttpContext;
pub use self::metadata::Metadata;
pub use self::rpc::RpcContext;
pub use self::standalone::StandaloneContext;
pub use self::ws::WsContext;

#[cfg(test)]
mod handle_bounds_tests {
    use super::*;

    fn assert_handle<T: Send + Sync + Clone + 'static>() {}

    /// Every enhancer signature takes `&C` across an await, and `&T` is `Send`
    /// only where `T` is `Sync`. A context losing `Sync` breaks the whole
    /// enhancer surface, and the error would surface far from the cause.
    #[test]
    fn every_context_is_a_send_sync_clone_handle() {
        assert_handle::<HttpContext>();
        assert_handle::<RpcContext>();
        assert_handle::<WsContext>();
        assert_handle::<GrpcContext>();
    }
}
