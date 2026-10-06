//! Scenarios: streams. On a link whose `shapes` exclude the streamed ones, each asserts the
//! startup refusal instead.

use std::time::Duration;

use futures_util::{StreamExt, stream};

use crate::Broker;
use crate::cases::app::{COUNT, DOUBLE, Fixture, Mounts, SUM, refused_at_startup, streams, within};

const WAIT: Duration = Duration::from_secs(10);

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
