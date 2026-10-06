//! Scenarios: sse.

use crate::wire::{Exchange, same_as_reference, start};
use crate::{Host, Mode};

/// An SSE stream whose item fails, written as an `error` event that ends it.
pub async fn error_event<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/sse")).await;
    assert_eq!(reply.status, 200);
    assert!(reply.header("content-type").is_some_and(|value| value.starts_with("text/event-stream")));
    let text = reply.text();
    assert!(text.contains("data: one"), "the first event is missing: {text}");
    assert!(text.contains("event: error"), "the error event is missing: {text}");
    host.stop().await;
}
