//! Scenarios: payloads.

use crate::Broker;

/// A binary payload round trip where the link declares `binary`, the pre-I/O refusal with `binary_unsupported` where not.
pub async fn binary<B: Broker>() {
    todo!()
}

/// An oversized payload, as `bad_request` with `payload_too_large`.
pub async fn oversized<B: Broker>() {
    todo!()
}
