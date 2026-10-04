//! Scenarios: drain.

use crate::{Host, Mode};

/// The drain: `Connection: close` on HTTP/1.1, GOAWAY on HTTP/2 where the host serves it, the connection closing after the 503.
pub async fn drain<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
