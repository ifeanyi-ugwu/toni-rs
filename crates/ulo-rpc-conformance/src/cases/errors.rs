//! Scenarios: errors.

use std::future::IntoFuture;
use std::time::Duration;

use futures_util::StreamExt;
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{
    ADD, BOOM, CONFLICT, Fixture, GUARDED, SUBSTITUTE, SUBSTITUTE_ITEMS, SUBSTITUTED, SUBSTITUTED_STREAM, Sum,
    UNENCODABLE, UNENCODABLE_STREAM, failed, streams, within,
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

/// A reply the link's codec cannot encode, the server's own failure: answered `internal` under the
/// call's id at once, never left to the caller's own `Timeout`; on a link carrying streamed
/// replies, an item it cannot encode ends the stream the same way after the items before it.
pub async fn unencodable_reply<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    // Twice the wait, so a server that sends nothing fails with the caller's `Timeout` rather
    // than with this harness's own limit.
    let call = fixture.rpc().request::<_, u32>(UNENCODABLE, &()).timeout(WAIT);
    let outcome = within(WAIT * 2, "the unencodable call", call.into_future()).await;
    failed(outcome, ErrorKind::Internal);
    if streams(&fixture.capabilities()) {
        let replies = fixture.rpc().stream::<_, u32>(UNENCODABLE_STREAM, &()).timeout(WAIT).await.expect("the stream opens");
        let items: Vec<_> = within(WAIT * 2, "the unencodable stream", replies.collect()).await;
        match items.as_slice() {
            [Ok(1), Err(error)] if error.kind() == ErrorKind::Internal => {}
            other => panic!("expected the item `1` and then an `internal` error, got: {other:?}"),
        }
    }
    fixture.stop().await;
}
