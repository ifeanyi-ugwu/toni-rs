//! What a dropped future already covers, and therefore what a cancellation token is not for.
//!
//! A Node promise keeps running after its consumer goes away, which is why Nest hands handlers an
//! `AbortSignal`. A Rust future stops when dropped. These pin that difference, since every decision
//! about cancellation in this framework rests on it — see ADR 0021.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serial_test::serial;
use toni::{controller, get, module, routes, Body as ToniBody};

use crate::common::TestServer;

static HANDLER_DROPPED: AtomicBool = AtomicBool::new(false);

struct Sentinel;

impl Drop for Sentinel {
    fn drop(&mut self) {
        HANDLER_DROPPED.store(true, Ordering::SeqCst);
    }
}

#[controller("/probe")]
pub struct ProbeController {}

#[routes]
impl ProbeController {
    /// Holds a sentinel across an await long enough for the client to give up.
    #[get("/slow")]
    async fn slow(&self) -> ToniBody {
        let _sentinel = Sentinel;
        tokio::time::sleep(Duration::from_secs(5)).await;
        ToniBody::text("never reached")
    }
}

#[module(controllers: [ProbeController])]
impl ProbeModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_disconnect_drops_the_handler_future() {
    HANDLER_DROPPED.store(false, Ordering::SeqCst);
    let server = TestServer::start(ProbeModule).await;

    // Give up well before the handler finishes, then drop the connection.
    let res = tokio::time::timeout(
        Duration::from_millis(300),
        server.client().get(server.url("/probe/slow")).send(),
    )
    .await;
    assert!(res.is_err(), "the request must not complete");

    for _ in 0..40 {
        if HANDLER_DROPPED.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        HANDLER_DROPPED.load(Ordering::SeqCst),
        "the handler future should be dropped when the client goes away"
    );
}

/// Negative control: a client that stays connected must not see the handler dropped, or the test
/// above would pass for a reason that has nothing to do with disconnecting.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_live_connection_does_not_drop_the_handler() {
    HANDLER_DROPPED.store(false, Ordering::SeqCst);
    let server = TestServer::start(ProbeModule).await;

    // The request future is held, never dropped: `timeout` would drop it, and dropping it closes
    // the connection, which is the very thing this is controlling for.
    let request = server.client().get(server.url("/probe/slow")).send();
    tokio::pin!(request);
    let waited = tokio::time::sleep(Duration::from_millis(800));
    tokio::pin!(waited);

    tokio::select! {
        _ = &mut request => panic!("the handler sleeps far longer than this"),
        _ = &mut waited => {}
    }

    assert!(
        !HANDLER_DROPPED.load(Ordering::SeqCst),
        "nothing should be dropped while the client is still connected"
    );
    drop(request);
}
