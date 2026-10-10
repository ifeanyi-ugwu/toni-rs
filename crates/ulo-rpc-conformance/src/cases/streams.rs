//! Scenarios: streams. On a link whose `shapes` exclude the streamed ones, each asserts the
//! startup refusal instead.

use std::time::Duration;

use futures_util::{StreamExt, stream};

use crate::Broker;
use crate::cases::app::{COLLECT, COUNT, DOUBLE, Fixture, Mounts, SUM, refused_at_startup, streams, within};

const WAIT: Duration = Duration::from_secs(10);

/// The items of the ordered client stream: enough that frames sent back to back by a link that
/// writes each from a task of its own would arrive out of order.
const ITEMS: u32 = 256;

/// The ordered client stream's own timeout: on Kafka each item waits for its delivery report.
const ORDERED_WAIT: Duration = Duration::from_secs(60);

/// A server stream, in order, ending with `end`.
pub async fn server_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let replies = fixture.rpc().stream::<_, u32>(COUNT, &3u32).timeout(WAIT).await.expect("the stream opens");
    let items: Vec<_> = within(WAIT, "the server stream", replies.collect()).await;
    let items: Vec<u32> = items.into_iter().map(|item| item.expect("every item arrives")).collect();
    assert_eq!(items, vec![1, 2, 3]);
    fixture.stop().await;
}

/// A client stream, answered by one reply.
pub async fn client_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let total = fixture.rpc().send_stream::<_, i64>(SUM, stream::iter(vec![1i64, 2, 3, 4])).timeout(WAIT).await;
    assert_eq!(total.expect("the client stream is answered"), 10);
    fixture.stop().await;
}

/// A client stream of many items, sent back to back, reaches the handler in the order it was sent,
/// with `in_end` last: the handler answers what arrived before `in_end`, which has to be every item
/// in order. An item overtaking another shows as a reordered answer, and `in_end` overtaking an
/// item as a short one.
pub async fn client_stream_in_order<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let sent: Vec<u32> = (0..ITEMS).collect();
    let arrived = fixture.rpc().send_stream::<_, Vec<u32>>(COLLECT, stream::iter(sent.clone())).timeout(ORDERED_WAIT).await;
    let arrived = arrived.expect("the ordered client stream is answered");
    if arrived != sent {
        let first = sent.iter().zip(&arrived).position(|(sent, arrived)| sent != arrived);
        panic!(
            "the server received {} of {ITEMS} items before `in_end`, first out of place at {first:?}: {arrived:?}",
            arrived.len()
        );
    }
    fixture.stop().await;
}

/// A bidirectional stream.
pub async fn bidi_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let replies = fixture.rpc().duplex::<_, i64>(DOUBLE, stream::iter(vec![1i64, 2, 3])).timeout(WAIT).await.expect("the stream opens");
    let items: Vec<_> = within(WAIT, "the bidirectional stream", replies.collect()).await;
    let items: Vec<i64> = items.into_iter().map(|item| item.expect("every item arrives")).collect();
    assert_eq!(items, vec![2, 4, 6]);
    fixture.stop().await;
}
