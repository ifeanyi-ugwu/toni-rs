//! Scenarios: lifecycle.

use crate::{Host, Mode};

/// The 503 before `listen()` returns and after `close`, carrying `Routing::Unrouted`.
pub async fn unavailable<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
