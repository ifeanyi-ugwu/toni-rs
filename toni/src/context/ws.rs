use std::sync::Arc;

use parking_lot::Mutex;

use crate::http_helpers::RouteMetadata;
use crate::websocket::{WsClient, WsError, WsMessage};

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for WebSocket handlers.
///
/// One per inbound message (or per `connect`/`disconnect` lifecycle event).
/// The response is held inside a `Mutex` so the type stays `Sync` despite the
/// fact that `WsHandlerOutput::Stream` carries a non-`Sync` `BoxStream` —
/// streams themselves bypass this slot and flow through a separate channel
/// in the gateway dispatcher.
pub struct WsContext {
    pub(crate) shared: SharedState,
    pub(crate) client: WsClient,
    pub(crate) message: WsMessage,
    pub(crate) event: String,
    pub(crate) response: Mutex<Option<Result<Option<WsMessage>, WsError>>>,
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
            response: Mutex::new(None),
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
        self.response.lock().clone()
    }

    pub fn set_response(&self, response: Result<Option<WsMessage>, WsError>) {
        *self.response.lock() = Some(response);
    }

    pub fn take_response(&self) -> Option<Result<Option<WsMessage>, WsError>> {
        self.response.lock().take()
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
