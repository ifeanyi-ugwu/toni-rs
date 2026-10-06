//! Scenarios: routing ext.

use crate::wire::{Exchange, is_reference, start};
use crate::{Host, Mode};

/// `Routing` in the response extensions, read through the header the host's test middleware writes,
/// and on rocket from the request's local cache through the test fairing. The reference has no
/// host around the app to read it.
pub async fn routing<H: Host>(mode: Mode) {
    if is_reference::<H>() {
        return;
    }
    let host = start::<H>(mode).await;
    let hit = host.send(Exchange::get("/hit")).await;
    assert_eq!(hit.routing(), Some(format!("matched {}", host.route("/hit")).as_str()));
    let miss = host.send(Exchange::get("/nowhere")).await;
    assert_eq!(miss.routing(), Some("not-found"));
    host.stop().await;
}
