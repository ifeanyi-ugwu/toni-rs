//! Scenarios: misses.

use std::time::Duration;

use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, NOBODY, NOBODY_EVENT, Sum, failed, miss_wait};

/// A pattern nothing handles: `Unavailable` where the link declares `miss_signal`, the client's `Timeout` where it does not; either reason accepted.
pub async fn unhandled_pattern<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let outcome = fixture.rpc().request::<_, ()>(NOBODY, &()).timeout(miss_wait(&fixture.broker)).await;
    if fixture.capabilities().miss_signal {
        let error = failed(outcome, ErrorKind::Unavailable);
        assert!(
            matches!(error.reason(), Some("pattern_unhandled" | "no_destination")),
            "a miss names `pattern_unhandled` or `no_destination`, not {:?}",
            error.reason()
        );
    } else {
        failed(outcome, ErrorKind::Timeout);
    }
    fixture.stop().await;
}

/// An event nothing handles is logged and acknowledged, so it cannot loop on redelivery.
pub async fn unhandled_event<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    match fixture.rpc().emit(NOBODY_EVENT, &1u32).await {
        Ok(()) => {}
        Err(error) => assert_eq!(error.kind(), ErrorKind::Unavailable, "an unhandled event fails only as `Unavailable`"),
    }
    tokio::time::sleep(fixture.broker.budget().settle).await;
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(Duration::from_secs(5)).await;
    assert_eq!(sum.expect("the server still answers after an unhandled event"), Sum { sum: 2 });
    fixture.stop().await;
}
