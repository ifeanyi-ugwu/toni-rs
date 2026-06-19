//! End-to-end integration tests for the salvo adapter.
//!
//! Each test boots a real server bound to an OS-assigned port, exercises a
//! single behavior over the wire, and tears down. The point is to validate
//! the salvo↔toni boundary — request adaptation, body streaming, WS upgrade,
//! routing — not the toni framework itself.

use futures_util::{SinkExt, StreamExt};
use toni::extractors::{BodyStream, Bytes, Path, Query};
use toni::toni_factory::ToniFactory;
use toni::*;
use toni_macros::{module, new, subscriptions, websocket_gateway};
use toni_salvo::SalvoAdapter;

#[derive(Debug, serde::Deserialize)]
struct SearchParams {
    q: String,
}

#[controller("/api")]
pub struct ApiController;

#[routes]
impl ApiController {
    #[get("/hello")]
    fn hello(&self) -> Body {
        Body::text("hello")
    }

    #[get("/users/:id")]
    fn user(&self, id: Path<String>) -> Body {
        Body::text(format!("user {}", id.0))
    }

    #[get("/search")]
    fn search(&self, q: Query<SearchParams>) -> Body {
        Body::text(format!("q={}", q.0.q))
    }

    #[post("/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::text(format!("echo:{}", body.0.len()))
    }

    #[post("/count")]
    async fn count(&self, body: BodyStream) -> Body {
        let mut total = 0u64;
        let mut s = Box::pin(body.into_stream());
        while let Some(chunk) = s.next().await {
            if let Ok(b) = chunk {
                total += b.len() as u64;
            }
        }
        Body::text(format!("count:{}", total))
    }
}

#[websocket_gateway("/ws")]
pub struct EchoGateway {}
#[subscriptions]
impl EchoGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[websocket_gateway("/separate", port = 0)]
pub struct SeparateGateway {}
#[subscriptions]
impl SeparateGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("separate-pong").into())
    }
}

#[module(controllers: [ApiController], providers: [EchoGateway])]
impl HttpOnlyModule {}

#[module(controllers: [ApiController], providers: [EchoGateway, SeparateGateway])]
impl FullModule {}

struct Bound {
    http_addr: std::net::SocketAddr,
    ws_addr: Option<std::net::SocketAddr>,
}

async fn start(
    module: toni::module_helpers::module_enum::ModuleDefinition,
    with_ws_adapter: bool,
) -> Bound {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_http_adapter(SalvoAdapter::new(), 0, "127.0.0.1")
            .unwrap();
        if with_ws_adapter {
            app.use_websocket_adapter(SalvoAdapter::new()).unwrap();
        }
        let bound = app.bind().await.unwrap();
        let http = bound.http.expect("HTTP not bound");
        let ws = bound.websocket.first().copied();
        let _ = tx.send(Bound {
            http_addr: http,
            ws_addr: ws,
        });
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });
    rx.await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn http_get_path_param_query_route_through_salvo() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule::module_definition(), false).await;
            let base = format!("http://{}", bound.http_addr);
            let client = reqwest::Client::new();

            let r = client
                .get(format!("{}/api/hello", base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "hello");

            let r = client
                .get(format!("{}/api/users/42", base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "user 42");

            let r = client
                .get(format!("{}/api/search?q=salvo", base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "q=salvo");

            let r = client
                .get(format!("{}/api/missing", base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 404);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_post_buffered_and_streaming_bodies() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule::module_definition(), false).await;
            let base = format!("http://{}", bound.http_addr);
            let client = reqwest::Client::new();

            // Buffered (Bytes extractor → RequestBody::collect)
            let r = client
                .post(format!("{}/api/echo", base))
                .body("hello world")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "echo:11");

            // Streaming (BodyStream extractor — chunked transfer, 1 MiB)
            let payload = vec![0u8; 1024 * 1024];
            let r = client
                .post(format!("{}/api/count", base))
                .body(payload)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "count:1048576");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_same_port_upgrade_and_echo() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule::module_definition(), false).await;
            let url = format!("ws://{}/ws", bound.http_addr);

            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"event":"ping"}"#.to_string().into(),
            ))
            .await
            .unwrap();

            let msg = ws.next().await.unwrap().unwrap();
            let text = msg.to_text().unwrap();
            assert_eq!(text, "pong");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_separate_port_upgrade_and_echo() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(FullModule::module_definition(), true).await;
            let ws_addr = bound.ws_addr.expect("WS adapter not bound");
            let url = format!("ws://{}/separate", ws_addr);

            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"event":"ping"}"#.to_string().into(),
            ))
            .await
            .unwrap();

            let msg = ws.next().await.unwrap().unwrap();
            assert_eq!(msg.to_text().unwrap(), "separate-pong");
        })
        .await;
}
