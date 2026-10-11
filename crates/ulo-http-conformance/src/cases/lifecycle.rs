//! Scenarios: lifecycle.

use ulo::Signal;

use crate::wire::{Exchange, PATIENCE, not_a_timeout, start, within};
use crate::{Host, Mode};

/// After `close`, the app answers nothing as if it were serving: the host has stopped with it,
/// so a request finds no listener, or, where the host still answers, the 503 the closed handle
/// gives, with `Connection: close` and `Routing::Unrouted`.
///
/// The 503 before `listen()` returns is not reached through [`Host`]: `start` returns once the
/// app listens, and an adapter's `run` serves the host only after that.
pub async fn unavailable<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let closed = within(host.timer(), PATIENCE, host.app.close(Signal::new("suite"))).await;
    assert!(closed.is_some(), "the app did not close within {PATIENCE:?}");
    match host.request(&Exchange::get("/hit")).await {
        Ok(response) => {
            assert_eq!(response.status, 503, "the closed app answered a request");
            let connection = response.header("connection");
            assert!(connection.is_some_and(|value| value.eq_ignore_ascii_case("close")));
            if let Some(routing) = response.routing() {
                assert_eq!(routing, "unrouted");
            }
        }
        Err(error) => not_a_timeout(&error, "a request after `close`"),
    }
    host.stop().await;
}
