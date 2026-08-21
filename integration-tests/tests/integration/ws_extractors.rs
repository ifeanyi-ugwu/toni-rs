//! A WebSocket handler takes what it needs, in any order.
//!
//! The fixed `(WsClient, WsMessage)` pair is now the most common choice rather
//! than the only signature — those handlers still compile everywhere else in
//! this suite, which is the compatibility half of the same change.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::context::{Extensions, HandlerContext, WsContext};
use toni::extractors::Payload;
use toni::traits_helpers::Guard;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    injectable, module, new, subscribe_message, subscriptions, use_guards, websocket_gateway,
};

use crate::common::TestServer;

#[derive(Deserialize)]
pub struct PlaceOrder {
    item: String,
    qty: u32,
}

#[derive(Clone)]
pub struct Principal(String);

#[injectable]
pub struct StampGuard {}

#[async_trait]
impl Guard<WsContext> for StampGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        ctx.extensions().insert(Principal("erin".into()));
        true
    }
}

#[websocket_gateway("/ws-extractors")]
pub struct ExtractorGateway {}

#[subscriptions]
#[use_guards(StampGuard)]
impl ExtractorGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The message parsed, without naming the client it never uses.
    #[subscribe_message("place")]
    async fn place(&self, Payload(order): Payload<PlaceOrder>) -> WsHandlerResult {
        Ok(WsMessage::text(format!("{}x{}", order.item, order.qty)).into())
    }

    /// Three extractors, declared in an order the old fixed pair could not express.
    #[subscribe_message("whoami")]
    async fn whoami(&self, ext: Extensions, ctx: &WsContext, client: WsClient) -> WsHandlerResult {
        let principal = ext
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        // The context reaches the handler, and its declared metadata is empty because this gateway
        // annotates nothing. Empty means not annotated here, rather than not populated.
        let metadata_is_empty = ctx.route_metadata().is_none_or(|m| m.is_empty());
        Ok(WsMessage::text(format!(
            "{principal}/{metadata_is_empty}/{}",
            !client.id.is_empty()
        ))
        .into())
    }

    /// Takes nothing at all.
    #[subscribe_message("ping")]
    async fn ping(&self) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [StampGuard, ExtractorGateway])]
impl ExtractorModule {}

async fn roundtrip(port: u16, frame: &str) -> String {
    let url = format!("ws://127.0.0.1:{}/ws-extractors", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
    ws.next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap()
        .to_string()
}

#[tokio_localset_test::localset_test]
async fn a_handler_takes_only_the_payload() {
    let server = TestServer::start(ExtractorModule).await;
    let reply = roundtrip(server.port, r#"{"event":"place","item":"boots","qty":2}"#).await;
    assert_eq!(reply, "bootsx2");
}

#[tokio_localset_test::localset_test]
async fn extractors_compose_in_any_order() {
    let server = TestServer::start(ExtractorModule).await;
    let reply = roundtrip(server.port, r#"{"event":"whoami"}"#).await;
    assert_eq!(reply, "erin/true/true");
}

#[tokio_localset_test::localset_test]
async fn a_handler_can_take_nothing() {
    let server = TestServer::start(ExtractorModule).await;
    let reply = roundtrip(server.port, r#"{"event":"ping"}"#).await;
    assert_eq!(reply, "pong");
}
