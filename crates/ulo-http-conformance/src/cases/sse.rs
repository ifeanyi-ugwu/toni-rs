//! Scenarios: sse.

use crate::{Host, Mode};

/// An SSE stream whose item fails, written as an `error` event that ends it.
pub async fn error_event<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
