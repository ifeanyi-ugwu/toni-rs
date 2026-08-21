use std::sync::Arc;

use async_trait::async_trait;

use crate::context::WsContext;
use crate::http_helpers::{ExecutionResult, RouteMetadata};

use super::{DisconnectReason, WsClient, WsError, WsHandlerOutput};

/// The enhancer tokens a gateway declares, resolved once at registration. Gateway-level tokens apply
/// to every handler; each `handlers` entry adds tokens for one event. A flat descriptor instead of a
/// dozen accessor methods — the gateway macro builds it, the resolver reads it once.
#[derive(Default)]
pub struct GatewayEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub pipe_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub handlers: Vec<GatewayHandlerEnhancers>,
}

/// Per-handler (per-event) enhancer tokens, applied on top of the gateway-level ones.
#[derive(Default)]
pub struct GatewayHandlerEnhancers {
    pub event: String,
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub pipe_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
}

/// Core gateway trait for WebSocket handlers
///
/// Gateways handle WebSocket connections and route messages to appropriate handlers.
/// They integrate with Toni's DI system and execution context for guards, interceptors,
/// and error handling.
#[async_trait]
pub trait GatewayTrait: Send + Sync {
    /// Get unique token for DI registration
    fn get_token(&self) -> String;

    /// Get WebSocket path (e.g., "/chat", "/notifications")
    fn get_path(&self) -> String;

    /// Get namespace (optional, for multi-tenancy)
    fn get_namespace(&self) -> Option<String> {
        None
    }

    /// Get the port this gateway listens on.
    ///
    /// `None` (default) means same port as the HTTP server.
    /// `Some(port)` triggers a separate WebSocket server on that port — requires a
    /// `WebSocketAdapter` to be registered via `ToniApplication::use_websocket_adapter()`.
    fn get_port(&self) -> Option<u16> {
        None
    }

    /// Called once after the gateway path is registered with the adapter, before any connections.
    async fn after_init(&self) {}

    /// Connection lifecycle: called when a client connects
    async fn on_connect(&self, client: &WsClient, context: &WsContext) -> Result<(), WsError> {
        // Default implementation: allow all connections
        let _ = (client, context);
        Ok(())
    }

    /// Connection lifecycle: called when a client disconnects
    /// Connection teardown. `context` is the disconnect's own execution, and is how the connection's
    /// [`Session`](crate::websocket::Session) is read one last time. No enhancers run here — a
    /// disconnect cannot be rejected.
    async fn on_disconnect(
        &self,
        client: &WsClient,
        reason: DisconnectReason,
        context: &WsContext,
    ) {
        // Default implementation: no-op
        let _ = (client, reason, context);
    }

    /// The JSON field name used to route incoming messages to a handler.
    ///
    /// Default: `"event"` — matches the standard `{"event":"...", ...}` convention.
    /// Override to `"type"` for graphql-ws protocol compatibility.
    fn event_field(&self) -> &str {
        "event"
    }

    /// Route message to appropriate handler based on event name.
    ///
    /// `Ok(WsHandlerOutput)` for the success path (Empty / Single / Stream);
    /// `Err` carries the user's typed error so the dispatcher can fan
    /// observers + run the chain on it before falling back to
    /// `WsError::to_message`.
    async fn handle_event(&self, ctx: &WsContext) -> ExecutionResult<WsHandlerOutput, WsError>;

    /// What the gateway declares for every handler, before any handler adds to it.
    fn get_route_metadata(&self) -> Arc<RouteMetadata> {
        Arc::new(RouteMetadata::new())
    }

    /// Per-event metadata for handlers that declare their own, already merged over the gateway's.
    /// An event absent from this list reads [`get_route_metadata`](Self::get_route_metadata).
    fn handler_metadata(&self) -> Vec<(String, Arc<RouteMetadata>)> {
        Vec::new()
    }

    /// All enhancer tokens for this gateway — gateway-level plus per-handler — resolved once at
    /// startup. Default is empty (a gateway with no declared enhancers).
    fn enhancers(&self) -> GatewayEnhancers {
        GatewayEnhancers::default()
    }
}
