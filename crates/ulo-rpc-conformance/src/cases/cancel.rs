//! Scenarios: cancel.

use std::time::Duration;

use futures_util::StreamExt;
use ulo::CancelReason;

use crate::Broker;
use crate::cases::app::{Fixture, Mounts, TICKS, eventually, refused_at_startup, streams, within};

/// `cancel` mid-stream fires `ClientCancelled` and stops the producer.
pub async fn mid_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let settle = fixture.broker.budget().settle;
    let mut ticks = fixture.rpc().stream::<_, u64>(TICKS, &()).timeout(Duration::from_secs(10)).await.expect("the stream opens");
    for _ in 0..2 {
        within(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields").expect("a tick arrives");
    }
    drop(ticks);
    let probe = fixture.server.probe.clone();
    assert!(eventually(settle.max(Duration::from_secs(2)), || probe.stopped()).await, "the producer kept running after `cancel`");
    assert_eq!(probe.cancelled(), Some(Some(CancelReason::ClientCancelled)));
    fixture.stop().await;
}
