use std::sync::Arc;

use async_trait::async_trait;

use crate::http_helpers::RouteMetadata;

use super::{RpcContext, RpcData, RpcError};

/// Core trait for RPC message handlers.
///
/// Mirrors [`GatewayTrait`] for WebSocket — one struct per RPC controller,
/// auto-discovered from the DI container. Implement via `#[rpc_controller]`.
///
/// [`GatewayTrait`]: crate::websocket::GatewayTrait
#[async_trait]
pub trait RpcControllerTrait: Send + Sync {
    fn get_token(&self) -> String;

    /// All patterns this controller handles (e.g. `["order.create", "order.list"]`).
    fn get_patterns(&self) -> Vec<String>;

    /// Route an incoming message to the right handler by `context.pattern`.
    ///
    /// Returns `Some(reply)` for request-response patterns (`#[message_pattern]`),
    /// or `None` for fire-and-forget events (`#[event_pattern]`).
    async fn handle_message(
        &self,
        data: RpcData,
        context: RpcContext,
    ) -> Result<Option<RpcData>, RpcError>;

    fn get_guard_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_interceptor_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_pipe_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_error_handler_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_route_metadata(&self) -> Arc<RouteMetadata> {
        Arc::new(RouteMetadata::new())
    }

    /// All patterns that have handler-level enhancers.
    ///
    /// Used by the resolver to pre-resolve per-handler enhancers at startup.
    fn get_handler_patterns(&self) -> Vec<String> {
        vec![]
    }

    fn get_handler_guard_tokens(&self, _pattern: &str) -> Vec<String> {
        vec![]
    }

    fn get_handler_interceptor_tokens(&self, _pattern: &str) -> Vec<String> {
        vec![]
    }

    fn get_handler_pipe_tokens(&self, _pattern: &str) -> Vec<String> {
        vec![]
    }

    fn get_handler_error_handler_tokens(&self, _pattern: &str) -> Vec<String> {
        vec![]
    }
}
