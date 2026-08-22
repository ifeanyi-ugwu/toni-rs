//! What a dropped future already covers, and therefore what a cancellation token is not for.
//!
//! A Node promise keeps running after its consumer goes away, which is why Nest hands handlers an
//! `AbortSignal`. A Rust future stops when dropped. These pin that difference, since every decision
//! about cancellation in this framework rests on it — see ADR 0021.

use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serial_test::serial;
use toni::context::{HandlerContext, HttpContext};
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

// ---- the tail a dropped future does not reach --------------------------------

/// Whether the spawned producer learned the client had gone.
static PRODUCER_SAW_CANCEL: AtomicBool = AtomicBool::new(false);
/// How many expensive units the producer completed after the client left.
static WORK_AFTER_DISCONNECT: AtomicUsize = AtomicUsize::new(0);

#[controller("/tail")]
pub struct TailController {}

#[routes]
impl TailController {
    /// Returns a stream fed by a spawned task, which is the shape the token exists for: the handler
    /// future is finished the moment this returns, so nothing drops the producer.
    #[get("/stream")]
    async fn stream(&self, ctx: &HttpContext) -> ToniBody {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let cancelled = ctx.cancellation().clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancelled.cancelled() => {
                        PRODUCER_SAW_CANCEL.store(true, Ordering::SeqCst);
                        break;
                    }
                    // Stands in for an expensive unit — a query, a page fetch. Without the arm
                    // above, each one runs to completion before the send that would reveal the
                    // client is gone.
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        WORK_AFTER_DISCONNECT.fetch_add(1, Ordering::SeqCst);
                        if tx.send(Ok(Bytes::from_static(b"tick"))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        ToniBody::stream(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

#[module(controllers: [TailController])]
impl TailModule {}

/// The producer stops when the body is dropped, rather than at whatever it was going to do next.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_dropped_body_cancels_the_work_feeding_it() {
    PRODUCER_SAW_CANCEL.store(false, Ordering::SeqCst);
    WORK_AFTER_DISCONNECT.store(0, Ordering::SeqCst);

    let server = TestServer::start(TailModule).await;
    let response = server
        .client()
        .get(server.url("/tail/stream"))
        .send()
        .await
        .expect("headers arrive before the body");

    // Take one frame, then abandon the body mid-stream.
    drop(response);

    tokio::time::sleep(Duration::from_millis(400)).await;

    assert!(
        PRODUCER_SAW_CANCEL.load(Ordering::SeqCst),
        "the task feeding the body must learn the client went away"
    );
}
