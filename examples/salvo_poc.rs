//! toni-salvo proof-of-concept
//!
//! Smoke test for the salvo adapter: HTTP routes, same-port WebSocket on
//! port 3001, and a separate-port WebSocket on port 3002.
//!
//! Run with: cargo run --example salvo_poc
//! Test:     curl http://127.0.0.1:3001/hello
//!           curl http://127.0.0.1:3001/hello/world
//!           websocat ws://127.0.0.1:3001/chat       # same-port WS
//!           websocat ws://127.0.0.1:3002/ping       # separate-port WS

use std::time::Duration;

use futures::stream;
use serde_json::json;
use toni::extractors::Path;
use toni::*;
use toni_macros::{module, websocket_gateway};
use toni_salvo::SalvoAdapter;

#[derive(Clone)]
pub struct HelloController;

#[controller("/hello")]
impl HelloController {
    pub fn new() -> Self {
        Self
    }

    #[get("/")]
    fn hello(&self) -> Body {
        Body::json(json!({ "message": "Hello from salvo!", "framework": "toni" }))
    }

    #[get("/:name")]
    fn hello_name(&self, name: Path<String>) -> Body {
        Body::json(json!({ "message": format!("Hello, {}!", name.0) }))
    }

    /// Streams three chunks with a 500ms gap between each. Used to verify the
    /// salvo adapter forwards body chunks incrementally rather than buffering.
    /// Path is `/hello/_/stream` to avoid colliding with `/hello/:name`.
    #[get("/_/stream")]
    async fn stream_demo(&self) -> Body {
        let s = stream::unfold(0u32, |n| async move {
            if n >= 3 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let chunk = bytes::Bytes::from(format!("chunk {n}\n"));
            Some((Ok::<_, std::io::Error>(chunk), n + 1))
        });
        Body::stream(s).with_content_type("text/plain; charset=utf-8")
    }
}

#[websocket_gateway("/chat", pub struct EchoGateway {})]
impl EchoGateway {
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("message")]
    async fn handle_message(
        &self,
        client: WsClient,
        message: WsMessage,
    ) -> WsHandlerResult {
        let text = message
            .as_text()
            .ok_or_else(|| WsError::InvalidMessage("Expected text message".into()))?;
        println!("[{}] {}", client.id, text);
        Ok(WsMessage::text(format!("Echo: {}", text)).into())
    }
}

#[websocket_gateway("/ping", port = 3002, pub struct PingGateway {})]
impl PingGateway {
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn handle_ping(
        &self,
        _client: WsClient,
        _msg: WsMessage,
    ) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(controllers: [HelloController], providers: [EchoGateway, PingGateway])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("toni-salvo PoC");
    println!("  HTTP   :3001 GET /hello, GET /hello/:name");
    println!("  WS     :3001 /chat        (same-port upgrade)");
    println!("  WS     :3002 /ping        (separate-port adapter)");

    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(SalvoAdapter::new(), 3001, "127.0.0.1")
        .unwrap();
    app.use_websocket_adapter(SalvoAdapter::new()).unwrap();

    app.start().await?;
    Ok(())
}
