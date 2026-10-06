//! Scenarios: recovery.

use std::time::{Duration, Instant};

use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, HOLD, Sum, within};

/// How long the call held across the disruption would take, were its connection kept.
const HELD_MS: u64 = 3_000;

/// A call waiting when `Broker::disrupt` severs the client's connection fails `Unavailable`, and
/// calls succeed again within `Budget::recovery`, the client connecting anew. A `disrupt` that
/// severs nothing lets the held call finish, which fails the scenario.
pub async fn after_disrupt<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let budget = fixture.broker.budget();
    let rpc = fixture.rpc().clone();
    let held = tokio::spawn(async move { rpc.request::<_, u64>(HOLD, &HELD_MS).timeout(Duration::from_secs(30)).await });
    // The held call reaches its handler before the connection is severed.
    tokio::time::sleep(budget.settle).await;
    within(budget.recovery, "`Broker::disrupt`", fixture.broker.disrupt()).await;
    let lost = within(Duration::from_millis(HELD_MS) + budget.recovery, "the call waiting on the severed connection", held)
        .await
        .expect("the held call's task completes");
    match lost {
        Err(error) if error.kind() == ErrorKind::Unavailable => {}
        other => panic!("a call waiting on a severed connection fails `Unavailable`, got: {other:?}"),
    }

    let deadline = Instant::now() + budget.recovery;
    loop {
        let outcome = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 4, b: 5 }).timeout(Duration::from_secs(1)).await;
        match outcome {
            Ok(sum) => {
                assert_eq!(sum, Sum { sum: 9 });
                break;
            }
            outcome if Instant::now() >= deadline => panic!("calls did not recover within the budget: {outcome:?}"),
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    fixture.stop().await;
}
