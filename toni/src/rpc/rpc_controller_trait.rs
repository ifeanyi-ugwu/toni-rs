use std::sync::Arc;

use async_trait::async_trait;

use crate::context::RpcContext;
use crate::http_helpers::{ExecutionResult, RouteMetadata};

use super::RpcData;

/// Core trait for RPC message handlers.
///
/// One struct per RPC controller, auto-discovered from the DI container.
/// Implement via `#[rpc_controller]`.
#[async_trait]
pub trait RpcControllerTrait: Send + Sync {
    fn get_token(&self) -> String;

    /// All patterns this controller handles.
    fn get_patterns(&self) -> Vec<String>;

    /// Route an inbound message to the right per-pattern handler.
    ///
    /// `Ok(Some(reply))` for request-response patterns (`#[message_pattern]`),
    /// `Ok(None)` for fire-and-forget events (`#[event_pattern]`), and
    /// `Err` carrying the user's typed error so the dispatcher can fan
    /// observers + run the chain on it before falling back to
    /// `RpcError::to_data`.
    async fn handle_message(
        &self,
        ctx: &RpcContext,
    ) -> ExecutionResult<Option<RpcData>, super::RpcError>;

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
