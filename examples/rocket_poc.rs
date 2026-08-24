//! toni-rocket proof-of-concept
//!
//! Smoke test for the rocket adapter: HTTP routes, response streaming, body
//! extractors, and a same-port WebSocket. Rocket buffers request bodies up
//! to 32 MiB before handing them to toni, so the `BodyStream` extractor
//! still works but won't show per-frame chunks.
//!
//! Run with: cargo run --example rocket_poc
//! Test:     curl http://127.0.0.1:3001/hello
//!           curl http://127.0.0.1:3001/hello/world
//!           curl -N http://127.0.0.1:3001/hello/_/stream
//!           websocat ws://127.0.0.1:3001/chat

use std::time::Duration;

use futures::stream;
use futures::StreamExt;
use serde_json::json;
use toni::extractors::{BodyStream, Bytes, Path};
use toni::*;
use toni_macros::{module, new, subscriptions, websocket_gateway};
use toni_rocket::RocketAdapter;

#[controller("/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    fn hello(&self) -> Body {
        Body::json(json!({ "message": "Hello from rocket!", "framework": "toni" }))
    }

    #[get("/{name}")]
    fn hello_name(&self, name: Path<String>) -> Body {
        Body::json(json!({ "message": format!("Hello, {}!", name.0) }))
    }

    /// Streams three chunks 500ms apart. Verifies response streaming through
    /// rocket's `streamed_body` bridge.
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

    #[post("/_/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::json(json!({ "received": body.0.len() }))
    }

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

#[module(controllers: [HelloController], providers: [EchoGateway])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("toni-rocket PoC");
    println!("  HTTP   :3001 GET /hello, GET /hello/{{name}}, GET /hello/_/stream");
    println!("  HTTP   :3001 POST /hello/_/echo, POST /hello/_/count");
    println!("  WS     :3001 /chat        (same-port upgrade via rocket_ws)");

    let mut app = ToniFactory::new().create_with(AppModule).await?;

    app.use_http_adapter(RocketAdapter::new(), ("127.0.0.1", 3001))
        .unwrap();

    app.start().await?;
    Ok(())
}
