//! toni-poem proof-of-concept
//!
//! Smoke test for the poem adapter: HTTP routes, response streaming,
//! both body extractors, same-port WebSocket on port 3001, and a
//! separate-port WebSocket on port 3002.
//!
//! Run with: cargo run --example poem_poc
//! Test:     curl http://127.0.0.1:3001/hello
//!           curl http://127.0.0.1:3001/hello/world
//!           curl -N http://127.0.0.1:3001/hello/_/stream
//!           websocat ws://127.0.0.1:3001/chat       # same-port WS
//!           websocat ws://127.0.0.1:3002/ping       # separate-port WS

use std::time::Duration;

use futures::stream;
use futures::StreamExt;
use serde_json::json;
use toni::extractors::{BodyStream, Bytes, Path};
use toni::*;
use toni_macros::{module, new, subscriptions, websocket_gateway};
use toni_poem::PoemAdapter;

#[controller("/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    fn hello(&self) -> Body {
        Body::json(json!({ "message": "Hello from poem!", "framework": "toni" }))
    }

    #[get("/{name}")]
    fn hello_name(&self, name: Path<String>) -> Body {
        Body::json(json!({ "message": format!("Hello, {}!", name.0) }))
    }

    /// Streams three chunks with a 500ms gap between each. Verifies the poem
    /// adapter forwards body chunks incrementally rather than buffering.
    /// Path is `/hello/_/stream` to avoid colliding with `/hello/{name}`.
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

    /// Buffered body — `Bytes` extractor calls `RequestBody::collect`.
    #[post("/_/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::json(json!({ "received": body.0.len() }))
    }

    /// True streaming — `BodyStream` consumes frames without buffering.
    #[post("/_/count")]
    async fn count_chunks(&self, body: BodyStream) -> Body {
        let mut chunks = 0u32;
        let mut bytes = 0u64;
        let mut s = Box::pin(body.into_stream());
        while let Some(item) = s.next().await {
            if let Ok(b) = item {
                chunks += 1;
                bytes += b.len() as u64;
            }
        }
        Body::json(json!({ "chunks": chunks, "bytes": bytes }))
    }
}

#[websocket_gateway("/chat")]
pub struct EchoGateway {}
#[subscriptions]
impl EchoGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("message")]
    async fn handle_message(&self, client: WsClient, message: WsMessage) -> WsHandlerResult {
        let text = message
            .as_text()
            .ok_or_else(|| WsError::InvalidMessage("Expected text message".into()))?;
        println!("[{}] {}", client.id, text);
        Ok(WsMessage::text(format!("Echo: {}", text)).into())
    }
}

#[websocket_gateway("/ping", port = 3002)]
pub struct PingGateway {}
#[subscriptions]
impl PingGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn handle_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(controllers: [HelloController], providers: [EchoGateway, PingGateway])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("toni-poem PoC");
    println!("  HTTP   :3001 GET /hello, GET /hello/{{name}}, GET /hello/_/stream");
    println!("  HTTP   :3001 POST /hello/_/echo, POST /hello/_/count");
    println!("  WS     :3001 /chat        (same-port upgrade)");
    println!("  WS     :3002 /ping        (separate-port adapter)");

    let mut app = ToniFactory::new().create_with(AppModule).await?;

    app.use_http_adapter(PoemAdapter::new(), ("127.0.0.1", 3001))
        .unwrap();
    app.use_websocket_adapter(PoemAdapter::new()).unwrap();

    app.start().await?;
    Ok(())
}
