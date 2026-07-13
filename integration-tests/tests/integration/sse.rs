use std::pin::Pin;
use std::time::Duration;

use crate::common::TestServer;
use futures_util::{stream, StreamExt};
use tokio::sync::broadcast;
use toni::{
    controller, extractors::Bytes, get, module, post, routes, sse, HttpResponse, Sse, SseEvent,
};
use toni_macros::{injectable, new};

// ── Service ──────────────────────────────────────────────────────────────────

#[injectable]
pub struct EventsService {
    tx: broadcast::Sender<String>,
}
impl EventsService {
    #[new]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self { tx }
    }

    pub fn emit(&self, data: String) {
        let _ = self.tx.send(data);
    }

    pub fn subscribe(
        &self,
    ) -> Pin<Box<dyn futures_util::stream::Stream<Item = SseEvent> + Send + Sync + 'static>> {
        let rx = self.tx.subscribe();
        Box::pin(stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(data) => return Some((SseEvent::data(data), rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }
}

// ── Controller ───────────────────────────────────────────────────────────────

#[controller("/sse")]
pub struct SseController {
    #[inject]
    events: EventsService,
}

#[routes]
impl SseController {
    #[get("/basic")]
    async fn basic(&self) -> impl toni::IntoResponse {
        sse(stream::iter([
            SseEvent::data("hello"),
            SseEvent::data("world"),
        ]))
    }

    #[get("/fields")]
    async fn fields(&self) -> impl toni::IntoResponse {
        sse(stream::iter([SseEvent::data("payload")
            .event("update")
            .id("42")
            .retry_ms(3000)]))
    }

    #[get("/multiline")]
    async fn multiline(&self) -> impl toni::IntoResponse {
        sse(stream::iter([SseEvent::data("line1\nline2\nline3")]))
    }

    #[get("/fallible")]
    async fn fallible(&self) -> impl toni::IntoResponse {
        Sse::new(stream::iter([Ok::<SseEvent, std::io::Error>(
            SseEvent::data("ok-event"),
        )]))
    }

    // Bounded to 2 events so the test connection closes after receiving them
    #[get("/live")]
    async fn live(&self) -> impl toni::IntoResponse {
        sse(self.events.subscribe().take(2))
    }

    #[sse("/attr-basic")]
    async fn attr_basic(&self) -> impl futures_util::Stream<Item = SseEvent> {
        stream::iter([SseEvent::data("hello"), SseEvent::data("world")])
    }

    #[sse("/attr-fallible")]
    async fn attr_fallible(
        &self,
    ) -> impl futures_util::Stream<Item = Result<SseEvent, std::io::Error>> {
        stream::iter([Ok(SseEvent::data("ok-event"))])
    }

    #[post("/emit")]
    async fn emit_event(&self, Bytes(data): Bytes) -> impl toni::IntoResponse {
        self.events
            .emit(String::from_utf8_lossy(&data).into_owned());
        HttpResponse::no_content().build()
    }
}

#[module(controllers: [SseController], providers: [EventsService])]
impl SseModule {}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio_localset_test::localset_test]
async fn test_sse_headers() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let resp = server
        .client()
        .get(server.url("/sse/basic"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct}"
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap(),
        "no-cache"
    );
    assert_eq!(
        resp.headers()
            .get("x-accel-buffering")
            .unwrap()
            .to_str()
            .unwrap(),
        "no"
    );
}

#[tokio_localset_test::localset_test]
async fn test_sse_basic_wire_format() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let body = server
        .client()
        .get(server.url("/sse/basic"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio_localset_test::localset_test]
async fn test_sse_event_fields() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let body = server
        .client()
        .get(server.url("/sse/fields"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains("id: 42\n"), "missing id line");
    assert!(body.contains("event: update\n"), "missing event line");
    assert!(body.contains("retry: 3000\n"), "missing retry line");
    assert!(body.contains("data: payload\n"), "missing data line");
}

#[tokio_localset_test::localset_test]
async fn test_sse_multiline_data() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let body = server
        .client()
        .get(server.url("/sse/multiline"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // Multi-line data must be split into separate "data:" lines per SSE spec
    assert_eq!(body, "data: line1\ndata: line2\ndata: line3\n\n");
}

#[tokio_localset_test::localset_test]
async fn test_sse_fallible_stream() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let body = server
        .client()
        .get(server.url("/sse/fallible"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: ok-event\n\n");
}

#[tokio_localset_test::localset_test]
async fn test_sse_broadcaster_delivers_to_subscriber() {
    let server = TestServer::start(SseModule::module_definition()).await;

    let live_url = server.url("/sse/live");
    let emit_url = server.url("/sse/emit");
    let client = server.client().clone();

    // Subscribe first, then emit concurrently. The /live handler takes 2 events
    // and closes, so the text() call completes once both events arrive.
    let (body, _, _) = tokio::join!(
        async {
            client
                .get(&live_url)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        },
        async {
            // Allow the subscription request to reach the server before emitting
            tokio::time::sleep(Duration::from_millis(100)).await;
            client.post(&emit_url).body("hello").send().await.unwrap();
        },
        async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            client.post(&emit_url).body("world").send().await.unwrap();
        },
    );

    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio_localset_test::localset_test]
async fn test_sse_attr_macro_infallible() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let resp = server
        .client()
        .get(server.url("/sse/attr-basic"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio_localset_test::localset_test]
async fn test_sse_attr_macro_fallible() {
    let server = TestServer::start(SseModule::module_definition()).await;
    let body = server
        .client()
        .get(server.url("/sse/attr-fallible"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: ok-event\n\n");
}
