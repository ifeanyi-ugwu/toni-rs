use async_trait::async_trait;

use crate::context::RpcContext;
use crate::http_helpers::ExecutionResult;

use super::RpcHandlerOutput;

/// An RPC controller instance: it answers one message.
///
/// Everything the framework reads before a call arrives — patterns, enhancer tokens — lives on
/// [`RpcControllerSource`](super::RpcControllerSource) instead, so an instance is needed only for
/// the duration of a call. Implement via `#[patterns]`.
#[async_trait]
pub trait RpcControllerTrait: Send + Sync {
    /// Route an inbound message to the right per-pattern handler.
    ///
    /// `Ok(Single(reply))` or `Ok(Stream(..))` for request-response patterns
    /// (`#[message_pattern]`), `Ok(Empty)` for fire-and-forget events
    /// (`#[event_pattern]`), and `Err` carrying the user's typed error so the
    /// dispatcher can fan observers + run the chain on it before falling back
    /// to `RpcError::to_data`.
    async fn handle_message(
        &self,
        ctx: &RpcContext,
    ) -> ExecutionResult<RpcHandlerOutput, super::RpcError>;
}
