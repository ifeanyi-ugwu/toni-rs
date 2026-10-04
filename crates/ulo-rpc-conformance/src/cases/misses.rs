//! Scenarios: misses.

use crate::Broker;

/// A pattern nothing handles: `Unavailable` where the link declares `miss_signal`, the client's `Timeout` where it does not; either reason accepted.
pub async fn unhandled_pattern<B: Broker>() {
    todo!()
}

/// An event nothing handles is logged and acknowledged, so it cannot loop on redelivery.
pub async fn unhandled_event<B: Broker>() {
    todo!()
}
