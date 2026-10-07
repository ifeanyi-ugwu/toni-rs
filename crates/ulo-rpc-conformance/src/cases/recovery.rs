//! Scenarios: recovery.

use std::time::{Duration, Instant};

use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, HOLD, Sum, within};

/// How long after `Broker::disrupt` the held call's handler answers, were its connection kept.
const HELD_AFTER_DISRUPT: Duration = Duration::from_secs(3);

/// A call waiting when `Broker::disrupt` severs the client's connection fails `Unavailable`, and
/// calls succeed again within `Budget::recovery`, the client connecting anew. A `disrupt` that
/// severs nothing lets the held call finish, which fails the scenario.
///
/// On a link declaring `durable_replies` the held call is answered instead, the broker keeping its
/// reply for the reconnected client. There a `disrupt` that severs nothing would pass, so the
/// environment's `disrupt` shows the cut itself (`Relay::cut_for` answers how many connections it
/// closed), and holds the client out until after the reply is published. The scenario then reads
/// the outage the environment observed, `Broker::outage`, and requires the moment the handler
/// returned its answer, which the link publishes at once, to fall after the client's connections
/// closed and before the first reconnection went through.
pub async fn after_disrupt<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let budget = fixture.broker.budget();
    let durable = fixture.capabilities().durable_replies;
    let rpc = fixture.rpc().clone();
    // The handler answers `HELD_AFTER_DISRUPT` after the scenario's settle, so after `disrupt`
    // however long the environment settles.
    let held_for = budget.settle + HELD_AFTER_DISRUPT;
    let held_ms = u64::try_from(held_for.as_millis()).unwrap_or(u64::MAX);
    let held = tokio::spawn(async move { rpc.request::<_, u64>(HOLD, &held_ms).timeout(Duration::from_secs(60)).await });
    // The held call reaches its handler before the connection is severed.
    tokio::time::sleep(budget.settle).await;
    within(budget.recovery, "`Broker::disrupt`", fixture.broker.disrupt()).await;
    let ended = within(held_for + budget.recovery, "the call waiting on the severed connection", held)
        .await
        .expect("the held call's task completes");
    match (ended, durable) {
        (Err(error), false) if error.kind() == ErrorKind::Unavailable => {}
        (other, false) => panic!("a call waiting on a severed connection fails `Unavailable`, got: {other:?}"),
        (Ok(answered), true) => {
            assert_eq!(answered, held_ms, "the held call's answer");
            let outage = fixture.broker.outage().expect(
                "the link declares `durable_replies`, so `Broker::outage` reports when the client was out",
            );
            let reopened = outage.reopened.expect("the client reconnected to receive the answer, so the relay let a connection through");
            let published = fixture.server.probe.held_answered().expect("the held call's handler answered");
            assert!(
                outage.shut <= published && published < reopened,
                "the held call's answer is published while the client is out: published {:?} after the cut, the client \
                 reconnecting {:?} after it",
                published.saturating_duration_since(outage.shut),
                reopened.saturating_duration_since(outage.shut),
            );
        }
        (other, true) => panic!(
            "the link declares `durable_replies`, so a call waiting on a severed connection is answered once the client reconnects, got: {other:?}"
        ),
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
