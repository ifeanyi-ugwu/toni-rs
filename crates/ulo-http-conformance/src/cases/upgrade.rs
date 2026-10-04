//! Scenarios: upgrade.

use crate::{Host, Mode};

/// A 101 upgrade with one frame echoed, where the host declares `upgrades`; refused where not.
pub async fn echo<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
