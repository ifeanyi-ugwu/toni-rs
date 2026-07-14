//! Verifies that WsHandlerOutput::Stream is driven by the framework.
//!
//! A handler returns a finite stream of messages. The test connects, sends one
//! trigger message, then reads until the stream is exhausted and confirms every
//! item arrived in order.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use toni::module;
use toni::websocket::{WsClient, WsHandlerOutput, WsHandlerResult, WsMessage};
use toni_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;

#[websocket_gateway("/ws-stream")]
pub struct CountGateway {}
#[subscriptions]
impl CountGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Returns a stream of three numbered messages.
    #[subscribe_message("count")]
    async fn on_count(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        let items = futures_util::stream::iter(
            ["one", "two", "three"]
                .into_iter()
                .map(|s| WsMessage::text(s.to_string())),
        );
        Ok(WsHandlerOutput::Stream(Box::pin(items)))
    }
}

#[module(providers: [CountGateway])]
struct CountModule;

/// A Stream handler delivers all items to the client in order.
#[tokio_localset_test::localset_test]
async fn ws_stream_handler_delivers_all_items_in_order() {
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(CountModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/ws-stream", server.port);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(r#"{"event":"count"}"#.to_string().into()))
        .await
        .unwrap();

    let mut received = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(2));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        received.push(t.to_string());
                        if received.len() == 3 { break; }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
        }
    }

    assert_eq!(received, ["one", "two", "three"]);
}
