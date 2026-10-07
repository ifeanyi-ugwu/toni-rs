//! Scenarios: errors.

use crate::app::SPARE;
use crate::wire::{Exchange, same_as_reference, start};
use crate::{Host, Mode};

/// An error an error handler reshapes, rendered by its new kind: the handler's `conflict` arrives
/// as the global handler's `forbidden`.
pub async fn reshaped<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/fail")).await;
    assert_eq!(reply.status, 403);
    assert!(reply.text().contains("reshaped by the suite"), "the reshaped message is missing: {}", reply.text());
    host.stop().await;
}

/// A handler's domain error an error handler claims with a value of its own: the handler's
/// `not_found` arrives as the error handler's 200 and its JSON item.
pub async fn recovered<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/recover")).await;
    assert_eq!((reply.status, reply.text()), (200, format!("{{\"name\":\"{SPARE}\"}}")));
    assert!(
        reply.header("content-type").is_some_and(|value| value.starts_with("application/json")),
        "the recovered item is not JSON: {:?}",
        reply.header("content-type")
    );
    host.stop().await;
}
