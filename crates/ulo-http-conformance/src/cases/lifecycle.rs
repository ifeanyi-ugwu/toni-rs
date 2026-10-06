//! Scenarios: lifecycle.

use ulo::Signal;

use crate::wire::{Exchange, PATIENCE, start};
use crate::{Host, Mode};

/// After `close`, the app answers nothing as if it were serving: the host has stopped with it,
/// so a request finds no listener, or, where the host still answers, the 503 the closed handle
/// gives, with `Connection: close` and `Routing::Unrouted`.
///
/// The 503 before `listen()` returns is not reached through [`Host`]: `start` returns once the
/// app listens, and an adapter's `run` serves the host only after that.
pub async fn unavailable<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let url = host.url("/hit");
    let closed = tokio::time::timeout(PATIENCE, host.app.close(Signal::new("suite"))).await;
    assert!(closed.is_ok(), "the app did not close within {PATIENCE:?}");
    let client = reqwest::Client::builder().pool_max_idle_per_host(0).timeout(PATIENCE).build().expect("a client");
    if let Ok(response) = client.request(Exchange::get("/hit").method, &url).send().await {
        assert_eq!(response.status().as_u16(), 503, "the closed app answered a request");
        let connection = response.headers().get("connection").and_then(|value| value.to_str().ok());
        assert!(connection.is_some_and(|value| value.eq_ignore_ascii_case("close")));
        if let Some(routing) = response.headers().get(crate::ROUTING_HEADER) {
            assert_eq!(routing.to_str().ok(), Some("unrouted"));
        }
    }
    host.stop().await;
}
