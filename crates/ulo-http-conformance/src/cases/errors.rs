//! Scenarios: errors.

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
