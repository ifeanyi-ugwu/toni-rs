use std::sync::Arc;

use async_trait::async_trait;

use crate::context::RpcContext;
use crate::http_helpers::{ExecutionResult, RouteMetadata};

use super::RpcData;

/// The enhancer tokens an RPC controller declares, resolved once at registration. Controller-level
/// tokens apply to every handler; each `handlers` entry adds tokens for one pattern. A flat
/// descriptor instead of a dozen accessor methods — the macro builds it, the resolver reads it once.
#[derive(Default)]
pub struct RpcEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub pipe_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub handlers: Vec<RpcHandlerEnhancers>,
}

/// Per-handler (per-pattern) enhancer tokens, applied on top of the controller-level ones.
#[derive(Default)]
pub struct RpcHandlerEnhancers {
    pub pattern: String,
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub pipe_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
}

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
        ctx: &mut RpcContext,
    ) -> ExecutionResult<Option<RpcData>, super::RpcError>;

    fn get_route_metadata(&self) -> Arc<RouteMetadata> {
        Arc::new(RouteMetadata::new())
    }

    /// All enhancer tokens for this controller — controller-level plus per-handler — resolved once at
    /// startup. Default is empty (a controller with no declared enhancers).
    fn enhancers(&self) -> RpcEnhancers {
        RpcEnhancers::default()
    }
}
