//! Scenarios: extraction.

use crate::{Host, Mode};

/// Each extraction failure's status, 400, 413, 415 and 422, with its problem document.
pub async fn failures<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
