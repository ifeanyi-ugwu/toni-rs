//! The in-memory broadcast adapter on its own: a subscriber receives what is published after it
//! subscribed, in order, and one that falls more than 1024 broadcasts behind skips the oldest and
//! goes on receiving; a publish with no subscriber succeeds.

use bytes::Bytes;
use futures_util::StreamExt;
use ulo_ws::{Audience, BroadcastAdapter, InMemory, NodeId, Target};

fn target() -> Target {
    Target::new(None, Audience::All, Vec::new())
}

async fn publish(adapter: &InMemory, n: usize) {
    adapter.publish(target(), Bytes::from(n.to_string())).await.expect("an in-memory publish succeeds");
}

#[tokio::test]
async fn a_subscriber_receives_what_is_published_after_it_subscribed() {
    let adapter = InMemory::new();
    publish(&adapter, 0).await;
    let mut carried = adapter.subscribe(NodeId::current());
    for n in 1..=3 {
        publish(&adapter, n).await;
    }
    for n in 1..=3 {
        let (_, frame) = carried.next().await.expect("the subscription ended");
        assert_eq!(frame, Bytes::from(n.to_string()), "broadcast {n}, in order, and not the one before the subscription");
    }
}

#[tokio::test]
async fn a_subscriber_more_than_1024_behind_skips_the_oldest_and_goes_on() {
    let adapter = InMemory::new();
    let mut carried = adapter.subscribe(NodeId::current());
    for n in 0..1030 {
        publish(&adapter, n).await;
    }
    let (_, first) = carried.next().await.expect("the subscription ended");
    assert_eq!(first, Bytes::from("6"), "the oldest broadcast kept once the six before it were skipped");
    let mut last = first;
    for _ in 0..1023 {
        last = carried.next().await.expect("the subscription ended").1;
    }
    assert_eq!(last, Bytes::from("1029"), "the newest broadcast, after the 1024 kept");
    publish(&adapter, 1030).await;
    assert_eq!(carried.next().await.expect("the subscription ended").1, Bytes::from("1030"), "a broadcast after the skip");
}
