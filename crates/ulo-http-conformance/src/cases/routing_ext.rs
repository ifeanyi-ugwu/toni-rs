//! Scenarios: routing ext.

use crate::{Host, Mode};

/// `Routing` in the response extensions, and on rocket in the request's local cache through the test fairing.
pub async fn routing<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
