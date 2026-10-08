//! Scenarios: drain.

use std::time::Duration;

use futures_util::StreamExt;
use ulo::Signal;
use ulo_rpc::{Capabilities, RpcError};
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, EVENT, Fixture, HOLD, Sum, UNTIL_DRAIN, eventually, miss_wait, server, streams, within};

/// How long the drain-window scenario waits between its calls, so a link that answers at once is
/// not flooded for the length of the drain.
const PACE: Duration = Duration::from_millis(10);

/// The event emitted during the drain on a link that declares `holds_unserved`.
const HELD_EVENT: u32 = 11;

/// The drain: a new call refused `unavailable`, an in-flight call finishing, a stream ending on
/// `draining()`. A link declaring `holds_unserved` answers the new call with the caller's own
/// `Timeout`, and an event emitted during the drain reaches the next instance.
pub async fn drain<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let capabilities = fixture.capabilities();
    let settle = fixture.broker.budget().settle;
    let held_for = u64::try_from((settle * 2 + Duration::from_millis(300)).as_millis()).unwrap_or(u64::MAX);

    let rpc = fixture.rpc().clone();
    let held = tokio::spawn(async move { rpc.request::<_, u64>(HOLD, &held_for).timeout(Duration::from_secs(30)).await });
    let mut ticking = if streams(&capabilities) {
        let mut ticks = fixture.rpc().stream::<_, u64>(UNTIL_DRAIN, &()).timeout(Duration::from_secs(10)).await.expect("the stream opens");
        within(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields").expect("a tick arrives");
        Some(ticks)
    } else {
        None
    };
    // The held call reaches its handler before the drain starts.
    tokio::time::sleep(settle).await;

    let handle = fixture.server.handle.clone();
    let closing = tokio::spawn(async move { handle.close(Signal::new("conformance drain")).await });
    let draining = fixture.server.handle.clone();
    assert!(eventually(Duration::from_secs(5), || draining.is_draining()).await, "the server did not start draining");
    // The link's own close signal takes effect at the broker before the new call is sent.
    tokio::time::sleep(settle / 2).await;

    let refused = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(miss_wait(&fixture.broker)).await;
    match refused {
        Err(error) if error.kind() == ErrorKind::Unavailable => {}
        // A link without a miss signal declares the caller's own timeout as its answer to a call
        // nothing takes, as in the unhandled-pattern scenario; a link whose broker holds the call
        // for the next instance declares it too, and the held event below shows the hold.
        Err(error) if error.kind() == ErrorKind::Timeout && (!capabilities.miss_signal || capabilities.holds_unserved) => {}
        other => panic!("a call arriving during the drain is refused `Unavailable`, got: {other:?}"),
    }
    if capabilities.holds_unserved {
        fixture.rpc().emit(EVENT, &HELD_EVENT).await.expect("an event during the drain is published");
    }

    let held = within(Duration::from_secs(30), "the in-flight call", held).await.expect("the held call's task completes");
    assert_eq!(held.expect("the in-flight call finishes during the drain"), held_for);
    if let Some(ticks) = ticking.take() {
        let rest: Vec<_> = within(Duration::from_secs(10), "the draining stream", ticks.collect()).await;
        assert!(rest.iter().all(Result::is_ok), "the stream ended on an error rather than cleanly: {rest:?}");
    }
    let _ = closing.await;
    if capabilities.holds_unserved {
        let budget = fixture.broker.budget();
        let next = server(&fixture.broker).await;
        let probe = next.probe.clone();
        let held = eventually(budget.boot + budget.settle, || probe.events() > 0).await;
        next.stop().await;
        assert!(held, "the event emitted during the drain did not reach the next instance, though the link declares `holds_unserved`");
        assert_eq!(probe.last_event(), Some(HELD_EVENT));
    }
    fixture.stop().await;
}

/// Calls sent from the drain's first moment until the server has closed, with no call held to
/// keep the drain open: each is refused `unavailable`, since a link's drain returns only once
/// nothing more is on its way to the server and the server answers everything that reached it. A
/// link declaring `confirms_drain(false)` cannot promise that, and may answer one with the
/// caller's own `Timeout`; a call sent once the server has closed is still `unavailable` there, which shows
/// the server stopped taking calls. A link without `miss_signal`, or declaring `holds_unserved`,
/// answers a call nothing takes with the caller's `Timeout`, as in the drain scenario.
pub async fn window<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let capabilities = fixture.capabilities();
    let wait = miss_wait(&fixture.broker);

    let handle = fixture.server.handle.clone();
    let mut closing = tokio::spawn(async move { handle.close(Signal::new("conformance drain window")).await });
    let draining = fixture.server.handle.clone();
    // Polled without a sleep, so the first call leaves as close to the drain's start as it can.
    let started = within(Duration::from_secs(5), "the server's drain", async {
        while !draining.is_draining() {
            tokio::task::yield_now().await;
        }
    });
    started.await;

    let mut during = Vec::new();
    within(Duration::from_secs(30), "the server's close", async {
        loop {
            let answer = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(wait).await;
            during.push(answer.map_err(|error| error.kind()));
            tokio::select! {
                biased;
                _ = &mut closing => return,
                () = tokio::time::sleep(PACE) => {}
            }
        }
    })
    .await;
    let after = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(wait).await;

    for (index, answer) in during.iter().enumerate() {
        let declared = answer == &Err(ErrorKind::Unavailable) || (answer == &Err(ErrorKind::Timeout) && times_out_during_drain(&capabilities));
        assert!(declared, "call {index} of {} sent during the drain was not refused `Unavailable`, got: {answer:?}", during.len());
    }
    assert_refused_after_close(&capabilities, after);
    fixture.stop().await;
}

/// Whether the caller's own `Timeout` is a declared answer for a call reaching a draining server.
fn times_out_during_drain(capabilities: &Capabilities) -> bool {
    !capabilities.miss_signal || capabilities.holds_unserved || !capabilities.confirms_drain
}

fn assert_refused_after_close(capabilities: &Capabilities, after: Result<Sum, RpcError>) {
    match after {
        Err(error) if error.kind() == ErrorKind::Unavailable => {}
        Err(error) if error.kind() == ErrorKind::Timeout && (!capabilities.miss_signal || capabilities.holds_unserved) => {}
        other => panic!("a call sent once the drained server had closed was not refused `Unavailable`, got: {other:?}"),
    }
}
