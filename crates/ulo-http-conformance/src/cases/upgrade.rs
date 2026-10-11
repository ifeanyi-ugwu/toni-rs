//! Scenarios: upgrade.

use crate::wire::{start, status_of};
use crate::{Host, Mode};

/// A 101 upgrade with one frame echoed, where the host declares `upgrades`. Where it does not, the
/// app carries no upgrade handler, `prepare` refusing one, and the same request must not switch
/// protocols.
pub async fn echo<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = host.raw().await;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: suite\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
        host.target("/echo")
    );
    raw.write(request.as_bytes()).await;
    let head = raw.read_until(b"\r\n\r\n", "the upgrade request").await.expect("the host answers the upgrade request");
    let status = status_of(&head);
    if H::limits().upgrades {
        assert_eq!(status, Some(101), "the host declares `upgrades` and did not switch protocols");
        raw.write(b"ping").await;
        let echoed = raw.read_until(b"ping", "the echoed frame").await;
        assert!(echoed.is_some(), "the upgraded connection did not echo the frame");
    } else {
        assert_ne!(status, Some(101), "the host declares `upgrades: false` and switched protocols");
    }
    host.stop().await;
}
