//! What "request scope" means on a WebSocket: one instance per message, not per connection.

use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::context::{HandlerContext, WsContext};
use toni::traits_helpers::Guard;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    injectable, module, new, subscribe_message, subscriptions, use_guards, websocket_gateway,
};

use crate::common::TestServer;

static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct SeenId(u64);

/// One per execution, numbered in construction order.
#[injectable(scope = "request")]
pub struct CallId {
    #[default(NEXT.fetch_add(1, Ordering::SeqCst))]
    pub id: u64,
}

/// Request-scoped because it holds a request-scoped dependency, which is what puts it on the
/// per-execution factory path rather than being built once at startup.
#[injectable(scope = "request")]
pub struct StampCallId {
    #[inject]
    call: CallId,
}

#[async_trait]
impl Guard<WsContext> for StampCallId {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        ctx.extensions().insert(SeenId(self.call.id));
        true
    }
}

#[websocket_gateway("/ws-request-scope")]
pub struct ScopeGateway {}

#[subscriptions]
#[use_guards(StampCallId)]
impl ScopeGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("which")]
    async fn which(&self, ctx: &WsContext, _client: WsClient) -> WsHandlerResult {
        let id = ctx.extensions().get::<SeenId>().map(|s| s.0).unwrap_or(0);
        Ok(WsMessage::text(id.to_string()).into())
    }
}

#[module(providers: [CallId, StampCallId, ScopeGateway])]
impl ScopeModule {}

#[tokio_localset_test::localset_test]
async fn a_request_scoped_provider_is_rebuilt_for_every_message() {
    let server = TestServer::start(ScopeModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-request-scope", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let mut seen = Vec::new();
    for _ in 0..2 {
        ws.send(Message::Text(r#"{"event":"which"}"#.to_string().into()))
            .await
            .unwrap();
        seen.push(
            ws.next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap()
                .to_string(),
        );
    }

    assert_ne!(
        seen[0], seen[1],
        "two messages on one connection must get two instances, not one shared across the \
         connection: {seen:?}"
    );
}
