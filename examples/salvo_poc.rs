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
