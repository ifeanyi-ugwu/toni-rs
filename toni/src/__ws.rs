//! Bridge between a `#[websocket_gateway]` struct and its optional `#[subscriptions]` impl.
//!
//! `#[websocket_gateway]` emits `impl GatewayTrait for Struct` with `get_path`/`namespace`/`port`
//! baked from the attribute, and the behavior methods delegating to `Self::__toni_ws_*` at the
//! concrete type. `#[subscriptions]` shadows `__toni_ws_handle_event` / `__toni_ws_enhancers`; the
//! single-slot connection-hook macros (`#[on_connect]` / `#[on_disconnect]` / `#[after_init]`) each
//! shadow their one forwarder. Whatever isn't shadowed falls to the defaults below, so a gateway with
//! neither is a valid connection-only gateway — the struct dispatches, it doesn't detect.

#![doc(hidden)]

use async_trait::async_trait;

use crate::context::Metadata;
use crate::context::WsContext;
use crate::http_helpers::ExecutionResult;
use crate::websocket::{DisconnectReason, GatewayEnhancers, WsClient, WsError, WsHandlerOutput};

/// Blanket "no handlers" defaults, implemented for every type. `#[subscriptions]` shadows these with
/// inherent fns of the same name, which win at the concrete-type call site in the generated
/// `GatewayTrait` impl.
#[async_trait]
pub trait WsHandlersBridge {
    async fn __toni_ws_after_init(&self) {}

    async fn __toni_ws_on_connect(
        &self,
        _client: &WsClient,
        _context: &WsContext,
    ) -> Result<(), WsError> {
        Ok(())
    }

    async fn __toni_ws_on_disconnect(
        &self,
        _client: &WsClient,
        _reason: DisconnectReason,
        _context: &WsContext,
    ) {
    }

    fn __toni_ws_metadata() -> crate::context::Metadata
    where
        Self: Sized,
    {
        crate::context::Metadata::new()
    }

    fn __toni_ws_handler_metadata() -> Vec<(String, crate::context::Metadata)>
    where
        Self: Sized,
    {
        Vec::new()
    }

    async fn __toni_ws_handle_event(
        &self,
        ctx: &WsContext,
    ) -> ExecutionResult<WsHandlerOutput, WsError> {
        // A typed event rather than a bare `WsError`, so a
        // `#[catch(Unrouted)]` handler can claim it. Unclaimed it renders
        // the same `NotFound` envelope it always did.
        ExecutionResult::Err(WsError::AppError(std::sync::Arc::new(
            crate::errors::Unrouted::new(ctx.event()),
        )))
    }

    fn __toni_ws_enhancers(&self) -> GatewayEnhancers {
        GatewayEnhancers::default()
    }
}

impl<T: ?Sized + Sync> WsHandlersBridge for T {}
