//! Scenarios: events.

use crate::Broker;
use crate::cases::app::{EVENT, Fixture, eventually};

/// An event reaches its handler, and nothing answers it.
pub async fn reaches_handler<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let settle = fixture.broker.budget().settle;
    fixture.rpc().emit(EVENT, &7u32).await.expect("the event is published");
    let probe = fixture.server.probe.clone();
    assert!(eventually(settle, || probe.events() == 1).await, "the event did not reach its handler within {settle:?}");
    assert_eq!(probe.last_event(), Some(7));
    // A second settle catches a redelivery or a second instance's copy.
    tokio::time::sleep(settle).await;
    assert_eq!(probe.events(), 1, "the event reached its handler more than once");
    fixture.stop().await;
}
