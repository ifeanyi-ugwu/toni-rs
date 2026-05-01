//! End-to-end integration tests for the rocket adapter.
//!
//! Boots a real server on an OS-assigned port (recovered via the liftoff
//! fairing) and exercises the rocket↔toni boundary over the wire — request
//! adaptation, body buffering, WebSocket upgrade, routing.
//!
//! Note: rocket buffers request bodies up to 32 MiB by default — `Data<'r>`
//! can't outlive the request, so unlike toni-axum/poem/salvo, the
//! `BodyStream` extractor sees a single chunk equal to the full payload
//! rather than per-frame chunks.

use futures_util::{SinkExt, StreamExt};
use toni::extractors::{BodyStream, Bytes, Path, Query};
use toni::toni_factory::ToniFactory;
use toni::*;
use toni_macros::{module, websocket_gateway};
use toni_rocket::RocketAdapter;

#[derive(Debug, serde::Deserialize)]
struct SearchParams {
    q: String,
}

#[controller("/api", pub struct ApiController;)]
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

#[websocket_gateway("/ws", pub struct EchoGateway {})]
impl EchoGateway {
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(controllers: [ApiController], providers: [EchoGateway])]
impl AppModule {}

struct Bound {
    http_addr: std::net::SocketAddr,
}

async fn start(module: toni::module_helpers::module_enum::ModuleDefinition) -> Bound {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_http_adapter(RocketAdapter::new(), 0, "127.0.0.1")
            .unwrap();
        let bound = app.bind().await.unwrap();
        let http = bound.http.expect("HTTP not bound");
        let _ = tx.send(Bound { http_addr: http });
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });
    rx.await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn http_get_path_param_query_route_through_rocket() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(AppModule::module_definition()).await;
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
                .get(format!("{}/api/search?q=rocket", base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "q=rocket");

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
async fn http_post_buffered_body_works() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(AppModule::module_definition()).await;
            let base = format!("http://{}", bound.http_addr);
            let client = reqwest::Client::new();

            let r = client
                .post(format!("{}/api/echo", base))
                .body("hello world")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "echo:11");

            // Rocket buffers the body before handing it to toni — `BodyStream`
            // still works (the 1 MiB body collects to the same number of
            // total bytes), it just doesn't expose per-network-frame chunks.
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
            let bound = start(AppModule::module_definition()).await;
            let url = format!("ws://{}/ws", bound.http_addr);

            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"event":"ping"}"#.to_string().into(),
            ))
            .await
            .unwrap();

            let msg = ws.next().await.unwrap().unwrap();
            assert_eq!(msg.to_text().unwrap(), "pong");
        })
        .await;
}
