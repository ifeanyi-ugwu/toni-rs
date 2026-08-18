use std::sync::Arc;

use crate::http_helpers::RouteMetadata;
use crate::websocket::{WsClient, WsMessage};

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for WebSocket handlers.
///
/// One per inbound message (or per `connect`/`disconnect` lifecycle event). A
/// handler answers by returning a [`WsHandlerOutput`](super::WsHandlerOutput),
/// streams included, so nothing about the answer lives here.
#[derive(Clone)]
pub struct WsContext {
    inner: Arc<WsInner>,
}

struct WsInner {
    shared: SharedState,
    client: WsClient,
    message: WsMessage,
    event: String,
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
            inner: Arc::new(WsInner {
                shared,
                client,
                message,
                event: event.into(),
            }),
        }
    }

    pub fn client(&self) -> &WsClient {
        &self.inner.client
    }

    pub fn message(&self) -> &WsMessage {
        &self.inner.message
    }

    pub fn event(&self) -> &str {
        &self.inner.event
    }
}

impl HandlerContext for WsContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.inner.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cache(&self) -> &crate::traits_helpers::ExecutionCache {
        &self.inner.shared.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }
}
