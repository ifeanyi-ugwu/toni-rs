use std::sync::Arc;

use crate::http_helpers::RouteMetadata;
use crate::websocket::{WsClient, WsMessage};

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for WebSocket handlers.
///
/// One per inbound message (or per `connect`/`disconnect` lifecycle event). A
/// handler answers by returning a [`WsHandlerOutput`](super::WsHandlerOutput),
/// streams included, so nothing about the answer lives here.
pub struct WsContext {
    pub(crate) shared: SharedState,
    pub(crate) client: WsClient,
    pub(crate) message: WsMessage,
    pub(crate) event: String,
}

impl WsContext {
    pub fn new(
        mut client: WsClient,
        message: WsMessage,
        event: impl Into<String>,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        let shared = SharedState::new(route_metadata);
        // The handler receives a clone of this client, not the context — pointing
        // the client at this message's bag is what carries an enhancer's work
        // across that boundary.
        client.extensions = shared.extensions.clone();
        Self {
            shared,
            client,
            message,
            event: event.into(),
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
}

impl HandlerContext for WsContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.shared.extensions
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
