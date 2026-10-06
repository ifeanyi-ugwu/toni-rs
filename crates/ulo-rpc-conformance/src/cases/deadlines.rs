//! Scenarios: deadlines.

use std::time::{Duration, Instant};

use ulo::CancelReason;
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{Fixture, STALL, eventually, failed};

/// `deadline-ms` fires `Deadline` and renders `timeout`.
pub async fn deadline_ms<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let outcome = fixture.rpc().request::<_, ()>(STALL, &()).header("deadline-ms", "300").timeout(Duration::from_secs(10)).await;
    failed(outcome, ErrorKind::Timeout);
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(eventually(settle, || probe.stalled().is_some()).await, "the stalled handler was not ended");
    assert_eq!(probe.stalled(), Some(Some(CancelReason::Deadline)));
    fixture.stop().await;
}

/// The client's own timeout, as `Timeout`.
pub async fn client_timeout<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let started = Instant::now();
    let outcome = fixture.rpc().request::<_, ()>(STALL, &()).timeout(Duration::from_millis(300)).await;
    failed(outcome, ErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3), "the client's timeout took {:?}", started.elapsed());
    fixture.stop().await;
}
