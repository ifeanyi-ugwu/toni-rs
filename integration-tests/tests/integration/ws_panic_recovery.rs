//! A panic inside a WebSocket handler is caught by the dispatcher,
//! surfaced as a `PanicRecovered` framework event, and rendered through
//! `WsError::to_message` — the connection stays alive and the next
//! message goes through normally. Sibling connections on the same gateway
//! are unaffected.

use toni::module;
use toni::websocket::{WsClient, WsError, WsHandlerResult, WsMessage};
use toni_macros::websocket_gateway;

use crate::common::TestServer;

#[websocket_gateway("/ws-panic-recovery", pub struct PanicGateway {})]
impl PanicGateway {
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("panic")]
    async fn on_panic(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        panic!("intentional test panic");
    }

    #[subscribe_message("safe")]
    async fn on_safe(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("safe-ok").into())
    }
}

#[module(providers: [PanicGateway])]
impl PanicGatewayModule {}

/// A handler panic surfaces as a canonical-envelope WS frame; the connection
/// stays open and the same client can send another message. Sibling
/// connections are unaffected.
///
/// Note: the test produces a "panicked at" line in stderr — that is the
/// Rust panic hook firing before catch_unwind catches the unwind. It is
/// expected and does not indicate a test failure.
#[tokio_localset_test::localset_test]
async fn ws_handler_panic_renders_envelope_and_keeps_connection_alive() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(PanicGatewayModule::module_definition()).await;
    let ws_url = format!("ws://127.0.0.1:{}/ws-panic-recovery", server.port);

    // Client A triggers the panic — receives the canonical envelope, not a Close.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws_a.send(Message::Text(r#"{"event":"panic"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws_a.next().await.unwrap().unwrap();
    let json: serde_json::Value =
        serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or_default()
            .contains("intentional test panic"),
        "panic message should surface in the envelope, got: {json}",
    );

    // Same connection still works for subsequent messages.
    ws_a.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws_a.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "safe-ok");

    // Client B (connected after the panic) reaches the safe handler normally.
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws_b.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws_b.next().await.unwrap().unwrap();
    assert_eq!(
        reply.to_text().unwrap(),
        "safe-ok",
        "sibling connections must be unaffected by another client's handler panic"
    );
}
