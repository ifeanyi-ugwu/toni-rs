//! Scenarios: errors.

use crate::Broker;

/// A handler's domain error, as the `err` envelope with its `kind`.
pub async fn domain_error<B: Broker>() {
    todo!()
}

/// A guard's refusal, as `forbidden`.
pub async fn guard_refusal<B: Broker>() {
    todo!()
}

/// A handler's panic, as `internal`.
pub async fn panic<B: Broker>() {
    todo!()
}

/// A `Payload<T>` that fails to decode, as `bad_request`.
pub async fn undecodable_payload<B: Broker>() {
    todo!()
}
