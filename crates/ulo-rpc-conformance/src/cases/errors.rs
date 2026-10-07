//! Scenarios: errors.

use std::time::Duration;

use futures_util::StreamExt;
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{
    ADD, BOOM, CONFLICT, Fixture, GUARDED, SUBSTITUTE, SUBSTITUTE_ITEMS, SUBSTITUTED, SUBSTITUTED_STREAM, Sum, failed,
    streams, within,
};

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

/// A handler's domain error an error handler claims with a value of its own, encoded by the link's
/// codec: the call is answered with that value, and, on a link carrying streamed replies, a
/// stream's error with the error handler's own stream of items.
pub async fn substituted<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let answer = fixture.rpc().request::<_, Sum>(SUBSTITUTED, &()).timeout(WAIT).await;
    assert_eq!(answer.expect("the error handler's value answers the call"), SUBSTITUTE);
    if streams(&fixture.capabilities()) {
        let replies = fixture.rpc().stream::<_, u32>(SUBSTITUTED_STREAM, &()).timeout(WAIT).await.expect("the stream opens");
        let items: Vec<_> = within(WAIT, "the error handler's stream", replies.collect()).await;
        let items: Vec<u32> = items.into_iter().map(|item| item.expect("every item arrives")).collect();
        assert_eq!(items, SUBSTITUTE_ITEMS);
    }
    fixture.stop().await;
}
