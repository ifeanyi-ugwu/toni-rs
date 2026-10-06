//! Scenarios: routing ext.

use crate::wire::{Exchange, is_reference, start};
use crate::{Host, Mode};

/// `Routing` in the response extensions, read through the header the host's test middleware writes,
/// and on rocket from the request's local cache through the test fairing. The reference has no
/// host around the app to read it, and declares the scenario not applicable.
pub async fn routing<H: Host>(mode: Mode) {
    assert!(!is_reference::<H>(), "the reference has no host to read `Routing`: declare `routing_extension` not applicable");
    let host = start::<H>(mode).await;
    let hit = host.send(Exchange::get("/hit")).await;
    assert_eq!(hit.routing(), Some(format!("matched {}", host.route("/hit")).as_str()));
    let miss = host.send(Exchange::get("/nowhere")).await;
    assert_eq!(miss.routing(), Some("not-found"));
    host.stop().await;
}
