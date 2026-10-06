//! Scenarios: errors.

use std::time::Duration;

use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, BOOM, CONFLICT, Fixture, GUARDED, Sum, failed};

const WAIT: Duration = Duration::from_secs(5);

/// A handler's domain error, as the `err` envelope with its `kind`.
pub async fn domain_error<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    failed(fixture.rpc().request::<_, ()>(CONFLICT, &()).timeout(WAIT).await, ErrorKind::Conflict);
    fixture.stop().await;
}

/// A guard's refusal, as `forbidden`.
pub async fn guard_refusal<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    failed(fixture.rpc().request::<_, ()>(GUARDED, &()).timeout(WAIT).await, ErrorKind::Forbidden);
    fixture.stop().await;
}

/// A handler's panic, as `internal`.
pub async fn panic<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    failed(fixture.rpc().request::<_, ()>(BOOM, &()).timeout(WAIT).await, ErrorKind::Internal);
    fixture.stop().await;
}

/// A `Payload<T>` that fails to decode, as `bad_request`.
pub async fn undecodable_payload<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    failed(fixture.rpc().request::<_, Sum>(ADD, &"not an object").timeout(WAIT).await, ErrorKind::BadRequest);
    fixture.stop().await;
}
