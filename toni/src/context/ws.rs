use std::sync::Arc;

use crate::http_helpers::RouteMetadata;
use crate::websocket::{WsClient, WsError, WsMessage};

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for WebSocket handlers.
///
/// One per inbound message (or per `connect`/`disconnect` lifecycle event). The
/// dispatcher owns the context and every enhancer borrows it exclusively, so
/// the response slot is a plain `Option` set through `&mut self`. Streams bypass
/// this slot and flow through a separate channel in the gateway dispatcher.
pub struct WsContext {
    pub(crate) shared: SharedState,
    pub(crate) client: WsClient,
    pub(crate) message: WsMessage,
    pub(crate) event: String,
    pub(crate) response: Option<Result<Option<WsMessage>, WsError>>,
}

impl WsContext {
    pub fn new(
        client: WsClient,
        message: WsMessage,
        event: impl Into<String>,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        Self {
            shared: SharedState::new(route_metadata),
            client,
            message,
            event: event.into(),
            response: None,
        }
    }

    pub fn client(&self) -> &WsClient {
        &self.client
    }

    pub fn message(&self) -> &WsMessage {
        &self.message
    }

    pub fn event(&self) -> &str {
        &self.event
    }

    pub fn response(&self) -> Option<Result<Option<WsMessage>, WsError>> {
        self.response.clone()
    }

    pub fn set_response(&mut self, response: Result<Option<WsMessage>, WsError>) {
        self.response = Some(response);
    }

    pub fn take_response(&mut self) -> Option<Result<Option<WsMessage>, WsError>> {
        self.response.take()
    }
}

impl HandlerContext for WsContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.shared.extensions
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.shared.cancellation
    }

    fn abort(&mut self) {
        self.shared.abort = true;
    }

    fn should_abort(&self) -> bool {
        self.shared.abort
    }
}
