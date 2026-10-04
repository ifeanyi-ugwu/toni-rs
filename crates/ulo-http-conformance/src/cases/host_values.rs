//! Scenarios: host values.

use crate::{Host, Mode};

/// `Host<T>` present, and absent as a 500, where the host declares `host_extensions`.
pub async fn present_and_absent<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}

/// An `Embedded::forward` copy reaching `Host<T>` on a host declaring `host_extensions: false`.
pub async fn forward_copy<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
