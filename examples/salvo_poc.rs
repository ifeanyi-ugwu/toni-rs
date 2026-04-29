//! toni-salvo proof-of-concept
//!
//! Smoke test for the salvo adapter: one HTTP route, one path-param route, and a
//! same-port WebSocket gateway echoing messages back to the sender.
//!
//! Run with: cargo run --example salvo_poc
//! Test:     curl http://127.0.0.1:3001/hello
//!           curl http://127.0.0.1:3001/hello/world
//!           websocat ws://127.0.0.1:3001/chat
//!           > {"event": "message", "data": "hi"}

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

#[module(controllers: [HelloController], providers: [EchoGateway])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("toni-salvo PoC running on http://127.0.0.1:3001");
    println!("  GET  /hello");
    println!("  GET  /hello/:name");
    println!("  WS   /chat");

    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(SalvoAdapter::new(), 3001, "127.0.0.1")
        .unwrap();

    app.start().await?;
    Ok(())
}
