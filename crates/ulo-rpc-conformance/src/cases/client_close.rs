//! Scenarios: closing the client.

use std::time::{Duration, Instant};

use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, NEVER, Sum, eventually, failed, within};

/// The waiting call's own timeout: longer than every bound below, so a call that fails within them
/// failed for the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the client's close has to show: at the waiting call, and at the environment's count of
/// the client's connections.
const PATIENCE: Duration = Duration::from_secs(5);

/// The client app's close, bounded so a close that hangs fails the scenario. A destroy hook that
/// outlasts its own bound, `hook_timeout`, fails it sooner, as a failure in the close's report.
const CLOSE_BOUND: Duration = Duration::from_secs(30);

/// Closing the client's app while a call waits on a handler that never answers reports no failure
/// and fails the call `Unavailable` within `PATIENCE`, the client module's destroy hook having
/// closed the link. Where the environment counts the client's connections
/// (`Broker::client_connections`), every one ends within `PATIENCE` and none opens again while the
/// client makes no call. A call the same client makes afterwards connects again and is answered.
///
/// The call is shown to be waiting at its handler, and still unanswered, before the close, so the
/// scenario fails on a close that leaves it waiting, the caller's own timeout lying beyond every
/// bound here, and on a call that failed for another reason first.
pub async fn client_close<B: Broker>() {
    let Fixture { broker, server, client } = Fixture::<B>::start().await;
    let budget = broker.budget();
    let rpc = client.rpc.clone();

    let caller = rpc.clone();
    let waiting = tokio::spawn(async move { caller.request::<_, ()>(NEVER, &()).timeout(CALL_TIMEOUT).await });
    let probe = server.probe.clone();
    assert!(
        eventually(budget.settle + PATIENCE, || probe.unanswered() > 0).await,
        "the call did not reach its handler within {:?}",
        budget.settle + PATIENCE,
    );
    let held = broker.client_connections().await;
    if let Some(open) = held {
        assert!(open > 0, "the environment counts no connection of the client's while its call waits");
    }
    assert!(!waiting.is_finished(), "the call ended before its client closed: {:?}", waiting.await);

    let closed = within(CLOSE_BOUND, "the client app's close", client.closed()).await;
    if let Err(error) = closed {
        panic!("the client app's close reported a failure: {error}");
    }
    let ended = within(PATIENCE, "the call waiting when its client closed", waiting).await.expect("the waiting call's task completes");
    failed(ended, ErrorKind::Unavailable);

    if held.is_some() {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match broker.client_connections().await {
                Some(0) => break,
                open if Instant::now() >= deadline => {
                    panic!("the client's connections were still open {PATIENCE:?} after its app closed: {open:?}")
                }
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        // A link reconnecting behind a closed client shows here: nothing has called since.
        tokio::time::sleep(budget.settle).await;
        let reopened = broker.client_connections().await;
        assert_eq!(reopened, Some(0), "the closed client opened a connection with no call made");
    }

    // A call after the close connects again. A broker can take a while to route replies to a new
    // connection, as a consumer group assigning its partitions, so the first calls may time out.
    let deadline = Instant::now() + budget.boot;
    loop {
        let outcome = rpc.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(Duration::from_millis(500)).await;
        match outcome {
            Ok(sum) => {
                assert_eq!(sum, Sum { sum: 5 });
                break;
            }
            outcome if Instant::now() >= deadline => {
                panic!("a call after the client's close was not answered within the boot budget: {outcome:?}")
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    if held.is_some() {
        let open = broker.client_connections().await;
        assert!(open.is_some_and(|open| open > 0), "the call after the close was answered with no connection open: {open:?}");
    }
    server.stop().await;
}
