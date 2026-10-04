//! Scenarios: pre dispatch.

use crate::{Host, Mode};

/// An unscoped entry answering a CORS preflight without calling `next`.
pub async fn preflight<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
