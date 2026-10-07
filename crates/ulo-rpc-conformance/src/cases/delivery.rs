//! Scenarios: delivery.

use ulo_rpc::{DeliveryMode, Link};

use crate::Broker;
use crate::cases::app::{Server, TALLY, client, eventually, ready, server};

/// Events the scenario publishes.
const EVENTS: usize = 10;

/// Two server instances under the link's declared `DeliveryMode`.
///
/// `Competing` delivers each event to one instance, `FanOut` to both. Under `Addressed` the caller
/// names one server, and a second cannot share its address, so the scenario runs one and asserts
/// it received every event.
pub async fn two_instances<B: Broker>() {
    let broker = B::start().await;
    let delivery = broker.link().capabilities().delivery;
    let first = server(&broker).await;
    let second = if delivery == DeliveryMode::Addressed { None } else { Some(server(&broker).await) };
    let servers: Vec<&Server> = std::iter::once(&first).chain(second.as_ref()).collect();
    let caller = client(&broker, &servers).await;
    ready(&broker, &caller.rpc, &servers).await;
    let budget = broker.budget();
    // The second instance's subscription settles before the events are published.
    tokio::time::sleep(budget.settle).await;

    for event in 0..EVENTS {
        caller.rpc.emit(TALLY, &(event as u32)).await.expect("the event is published");
    }
    let expected = match delivery {
        DeliveryMode::FanOut => 2 * EVENTS,
        _ => EVENTS,
    };
    let total = || first.probe.tallies() + second.as_ref().map_or(0, |second| second.probe.tallies());
    assert!(
        eventually(budget.boot + budget.settle, || total() >= expected).await,
        "{} of {expected} deliveries arrived under {delivery:?}",
        total()
    );
    tokio::time::sleep(budget.settle).await;
    assert_eq!(total(), expected, "deliveries under {delivery:?}");

    first.stop().await;
    if let Some(second) = second {
        second.stop().await;
    }
    caller.close().await;
}
