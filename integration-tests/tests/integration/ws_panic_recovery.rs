//! Verifies that a panic inside a WebSocket handler closes the connection
//! cleanly (disconnect callback fires, client gets EOF/close) instead of
//! leaving the connection dangling indefinitely.

use std::time::Duration;

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

/// A handler panic must close the connection promptly.
/// Sibling connections on the same gateway must be unaffected.
///
/// Note: the test produces a "panicked at" line in stderr — that is the
/// Rust panic hook firing before catch_unwind catches the unwind.
/// It is expected and does not indicate a test failure.
#[tokio_localset_test::localset_test]
async fn ws_handler_panic_closes_connection_and_leaves_siblings_unaffected() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(PanicGatewayModule::module_definition()).await;
    let ws_url = format!("ws://127.0.0.1:{}/ws-panic-recovery", server.port);

    // Client A triggers the panic — its connection must close.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws_a.send(Message::Text(r#"{"event":"panic"}"#.to_string().into()))
        .await
        .unwrap();

    let closed = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(msg) = ws_a.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => return true,
                _ => {}
            }
        }
        true // stream ended = closed
    })
    .await;
    assert!(
        closed.is_ok(),
        "panicking handler should close the connection within 500 ms"
    );

    // Client B connects after the panic — must reach the safe handler normally.
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
