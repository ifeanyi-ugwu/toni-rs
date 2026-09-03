//! A trait whose method and stream names do not pair, served in the handler
//! form.
//!
//! `#[grpc_stream]` reads the associated type's name off the method —
//! `greet_many` pairs with `GreetManyStream` — which is the pairing
//! tonic-build derives from one proto identifier. `tonic_build::manual` sets
//! the Rust name and the route name independently, so this fixture's `watch`
//! answers on `StreamProgress` and declares `StreamProgressStream`. The
//! attribute names it there.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use serial_test::serial;
use toni::context::{GrpcContext, HandlerContext};
use toni::extractors::Payload;
use toni::ToniFactory;
use toni::{module, ErrorKind};
use toni_macros::{controller, grpc_methods, new};

// The manual fixture in `build.rs` names its message types by path, and that
// path is the other test's module — so this file borrows those rather than
// including a second copy the trait would not accept.
use crate::grpc_stream_optin::msgs;

mod watch_svc {
    tonic::include_proto!("toni_test.watch.Watcher");
}

use watch_svc::watcher_client::WatcherClient;
use watch_svc::watcher_server::{Watcher, WatcherServer};

static SAW_CANCEL: AtomicBool = AtomicBool::new(false);

/// A handler's error type implements `toni::Error`. `GrpcStatus` does not —
/// it is what a `toni::Error` maps into — so a handler names its own.
#[derive(Debug)]
struct WatchFailed;

impl std::fmt::Display for WatchFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the watch could not start")
    }
}

impl std::error::Error for WatchFailed {}

impl toni::Error for WatchFailed {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Unavailable
    }
}

#[controller]
pub struct ManualWatcher {}

#[grpc_methods(watch_svc::watcher_server::Watcher)]
impl ManualWatcher {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The trait declares `StreamProgressStream` for a method called `watch`,
    /// so the pairing cannot be inferred and the attribute carries it.
    #[grpc_stream(StreamProgressStream)]
    async fn watch(
        &self,
        Payload(_req): Payload<msgs::WatchRequest>,
        ctx: &GrpcContext,
    ) -> Result<
        impl futures_util::Stream<Item = Result<msgs::ProgressEvent, WatchFailed>> + Send + 'static,
        WatchFailed,
    > {
        let context = ctx.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for _ in 0..200 {
                let _ = tx.send(Ok(msgs::ProgressEvent {
                    id: 1,
                    status: "tick".to_string(),
                }));
                tokio::select! {
                    _ = context.cancellation().cancelled() => {
                        SAW_CANCEL.store(true, Ordering::SeqCst);
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {}
                }
            }
        });
        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }
}

#[module(controllers: [ManualWatcher])]
impl ManualWatcherModule {}

async fn boot() -> u16 {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new()
            .create_with(ManualWatcherModule)
            .await
            .unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.unwrap()
}

/// Serving the route at all says the named associated type matched the trait's;
/// the abandoned tail says the wrapper still recognised the reply as streaming.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_named_associated_type_serves_a_manual_trait() {
    SAW_CANCEL.store(false, Ordering::SeqCst);
    let port = boot().await;

    let mut client = WatcherClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    );

    let mut stream = client
        .watch(msgs::WatchRequest { id: 1 })
        .await
        .expect("the call succeeds")
        .into_inner();

    stream.next().await.expect("one item").expect("an ok item");
    drop(stream);

    let mut fired = false;
    for _ in 0..40 {
        if SAW_CANCEL.load(Ordering::SeqCst) {
            fired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        fired,
        "the work feeding the reply must learn the caller went"
    );
}
