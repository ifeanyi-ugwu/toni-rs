//! Server-Sent Events (SSE) example
//!
//! Demonstrates four SSE patterns:
//!
//! 1. Counter  — a live stream using `stream::unfold`, emits a count every second
//! 2. Events   — named event types (client listens with `es.addEventListener`)
//! 3. Push     — a background task drives the stream via a per-request mpsc channel
//! 4. Broadcaster — a service-level broadcast channel: POST /sse/emit to push a
//!                  message to every client currently connected to GET /sse/live.
//!                  This is the Rust equivalent of NestJS's Subject + asObservable pattern.
//!
//! Run with:  cargo run --example sse
//!
//! Test in a terminal:
//!   curl -N http://127.0.0.1:3000/sse/counter
//!   curl -N http://127.0.0.1:3000/sse/events
//!   curl -N http://127.0.0.1:3000/sse/push
//!   curl -N http://127.0.0.1:3000/sse/live           (open two of these)
//!   curl -X POST http://127.0.0.1:3000/sse/emit -d 'hello world'
//!
//! Or in a browser:
//!   const es = new EventSource("http://127.0.0.1:3000/sse/counter");
//!   es.onmessage = (e) => console.log(e.data);

use std::pin::Pin;
use std::time::Duration;

use futures::stream;
use futures::Stream;
use tokio::sync::broadcast;
use toni::extractors::Bytes;
use toni::*;
use toni_axum::AxumAdapter;
use toni_macros::{injectable, new};

// ── Service ──────────────────────────────────────────────────────────────────

#[injectable]
pub struct EventsService {
    tx: broadcast::Sender<String>,
}
impl EventsService {
    #[new]
    pub fn new() -> Self {
        // The initial receiver is discarded; senders can still accept messages
        // and new receivers are created via `subscribe()` on each connection.
        let (tx, _) = broadcast::channel(64);
        Self { tx }
    }

    /// Push a message to all connected subscribers.
    pub fn emit(&self, data: String) {
        let _ = self.tx.send(data);
    }

    /// Subscribe to the broadcast. Each call returns an independent stream
    /// that receives every message emitted after the subscription point.
    pub fn subscribe(&self) -> Pin<Box<dyn Stream<Item = SseEvent> + Send + Sync + 'static>> {
        let rx = self.tx.subscribe();
        Box::pin(stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(data) => return Some((SseEvent::data(data), rx)),
                    // Slow consumer missed events — continue rather than disconnect
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
    /// Emits a count every second, forever.
    #[get("/counter")]
    async fn counter(&self) -> impl IntoResponse {
        let s = stream::unfold(0u32, |n: u32| async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Some((
                SseEvent::data(format!("count: {n}")).id(n.to_string()),
                n + 1,
            ))
        });
        sse(s)
    }

    /// Emits events with distinct names — clients can listen selectively:
    ///   es.addEventListener("ping", (e) => ...)
    ///   es.addEventListener("status", (e) => ...)
    #[get("/events")]
    async fn events(&self) -> impl IntoResponse {
        let s = stream::unfold(0u32, |n: u32| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let event = if n % 3 == 0 {
                SseEvent::data(format!("tick {n}")).event("ping")
            } else {
                SseEvent::data(format!(r#"{{"n":{n},"ok":true}}"#)).event("status")
            };
            Some((event, n + 1))
        });
        sse(s)
    }

    /// Push (per-request): a background task drives this specific connection.
    #[get("/push")]
    async fn push(&self) -> impl IntoResponse {
        let (tx, rx) = tokio::sync::mpsc::channel::<SseEvent>(16);

        tokio::spawn(async move {
            for i in 0..5u32 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = tx
                    .send(SseEvent::data(format!("push event {i}")).id(i.to_string()))
                    .await;
            }
            // tx dropped here — stream ends
        });

        let s = stream::unfold(
            rx,
            |mut rx: tokio::sync::mpsc::Receiver<SseEvent>| async move {
                rx.recv().await.map(|event| (event, rx))
            },
        );
        sse(s)
    }

    /// Live: service-level broadcaster — every connected client receives every emitted message.
    /// POST /sse/emit to push a message.
    #[get("/live")]
    async fn live(&self) -> impl IntoResponse {
        sse(self.events.subscribe())
    }

    /// Emit: push a message to all current /sse/live subscribers.
    #[post("/emit")]
    async fn emit_event(&self, Bytes(data): Bytes) -> impl IntoResponse {
        self.events
            .emit(String::from_utf8_lossy(&data).into_owned());
        HttpResponse::no_content().build()
    }
}

#[module(controllers: [SseController], providers: [EventsService])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 toni SSE example\n");
    println!("  GET  http://127.0.0.1:3000/sse/counter  — live counter (Ctrl-C to stop)");
    println!("  GET  http://127.0.0.1:3000/sse/events   — named event types");
    println!("  GET  http://127.0.0.1:3000/sse/push     — per-request background task (5 events)");
    println!("  GET  http://127.0.0.1:3000/sse/live     — service broadcaster (open multiple)");
    println!("  POST http://127.0.0.1:3000/sse/emit     — push to all /live subscribers");
    println!();
    println!("  Try opening two terminals:");
    println!("    curl -N http://127.0.0.1:3000/sse/live");
    println!("    curl -X POST http://127.0.0.1:3000/sse/emit -d 'hello everyone'");
    println!();

    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();

    app.start().await?;
    Ok(())
}
