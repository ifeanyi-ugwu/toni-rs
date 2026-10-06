//! Scenarios: disconnect.

use std::time::Duration;

use ulo_http::embed::Disconnect;

use crate::app::IDLE;
use crate::wire::{Raw, start};
use crate::{Host, Mode};

/// How long after the client leaves a host declaring `AtClose` has to observe it, well inside the
/// stream's idle period.
const AT_CLOSE: Duration = Duration::from_millis(300);

/// How long a host declaring `AtNextWrite` has, the stream writing again after its idle period.
const AT_NEXT_WRITE: Duration = Duration::from_secs(3);

/// A disconnect mid-stream firing `Disconnected` at the declared `Disconnect` moment. The stream
/// writes one event, idles for `IDLE`, then writes every 100 ms. The client leaves after the
/// first event: a host declaring `AtClose` must observe it during the idle period, and one
/// declaring `AtNextWrite` must not, but must once the stream writes again.
pub async fn mid_stream<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = Raw::connect(&host.authority()).await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\nAccept: text/event-stream\r\n\r\n", host.target("/endless"));
    raw.write(request.as_bytes()).await;
    raw.read_until(b"data: start", "the stream's first event").await.expect("the stream's first event arrives");
    drop(raw);

    tokio::time::sleep(AT_CLOSE).await;
    let early = host.probe.disconnected();
    match H::limits().disconnect {
        Disconnect::AtClose => {
            assert!(early, "the host declares `AtClose` and did not observe the disconnect while the stream was idle");
        }
        Disconnect::AtNextWrite => {
            assert!(!early, "the host declares `AtNextWrite` and observed the disconnect before the next write: declare `AtClose`");
            assert!(AT_CLOSE < IDLE);
            let deadline = tokio::time::Instant::now() + AT_NEXT_WRITE;
            while !host.probe.disconnected() {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the host declares `AtNextWrite` and did not observe the disconnect after the stream wrote again"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    host.stop().await;
}
