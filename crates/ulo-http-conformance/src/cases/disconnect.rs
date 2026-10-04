//! Scenarios: disconnect.

use crate::{Host, Mode};

/// A disconnect mid-stream firing `Disconnected` at the declared `Disconnect` moment.
pub async fn mid_stream<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
