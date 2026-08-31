//! `#[stream(...)]` — the signal for a trait whose own naming does not connect a
//! method to its stream.
//!
//! `#[grpc_methods]` reads a streaming method from its response type naming
//! `Self::SomeStream`, or from the method pairing with an associated type by
//! name. Both rest on tonic-build deriving the two names from one identifier,
//! which the proto path guarantees and `tonic_build::manual` does not: `name`
//! and `route_name` are independent there, so this fixture's `watch` answers on
//! the route `StreamProgress` and its associated type is `StreamProgressStream`.
//!
//! `WithOptIn` names it and is covered. `WithoutOptIn` is the same service
//! without the attribute, and is the control: if it were covered, the attribute
//! would be proving nothing.

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serial_test::serial;
use toni::context::{GrpcContext, HandlerContext};
use toni::ToniFactory;
use toni_macros::{controller, grpc_methods, module, new};

pub mod msgs {
    tonic::include_proto!("toni_test.orders");
}

mod watch_svc {
    tonic::include_proto!("toni_test.watch.Watcher");
}

use watch_svc::watcher_client::WatcherClient;
use watch_svc::watcher_server::{Watcher, WatcherServer};

type EventStream = Pin<Box<dyn Stream<Item = Result<msgs::ProgressEvent, tonic::Status>> + Send>>;

static OPT_IN_SAW_CANCEL: AtomicBool = AtomicBool::new(false);
static CONTROL_SAW_CANCEL: AtomicBool = AtomicBool::new(false);

/// Feeds the reply from outside the handler's future, ignoring a closed
/// receiver, so only the cancellation token can stop it.
fn detached_producer(context: GrpcContext, saw_cancel: &'static AtomicBool) -> EventStream {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for _ in 0..200 {
            let _ = tx.send(Ok(msgs::ProgressEvent {
                id: 1,
                status: "tick".to_string(),
            }));
            tokio::select! {
                _ = context.cancellation().cancelled() => {
                    saw_cancel.store(true, Ordering::SeqCst);
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    });
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

#[controller]
pub struct WithOptIn {}

impl WithOptIn {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Watcher for WithOptIn {
    type StreamProgressStream = EventStream;

    #[stream(StreamProgressStream)]
    async fn watch(
        &self,
        request: tonic::Request<msgs::WatchRequest>,
    ) -> Result<tonic::Response<EventStream>, tonic::Status> {
        let context =
            GrpcContext::of(request.extensions()).expect("dispatched through the framework");
        Ok(tonic::Response::new(detached_producer(
            context,
            &OPT_IN_SAW_CANCEL,
        )))
    }
}

#[module(controllers: [WithOptIn])]
impl WithOptInModule {}

#[controller]
pub struct WithoutOptIn {}

impl WithoutOptIn {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Watcher for WithoutOptIn {
    type StreamProgressStream = EventStream;

    async fn watch(
        &self,
        request: tonic::Request<msgs::WatchRequest>,
    ) -> Result<tonic::Response<EventStream>, tonic::Status> {
        let context =
            GrpcContext::of(request.extensions()).expect("dispatched through the framework");
        Ok(tonic::Response::new(detached_producer(
            context,
            &CONTROL_SAW_CANCEL,
        )))
    }
}

#[module(controllers: [WithoutOptIn])]
impl WithoutOptInModule {}

async fn boot<M>(module: M) -> (u16, toni::ShutdownHandle)
where
    M: toni::ModuleMetadata + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("grpc must bind").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn abandon_after_one_item(port: u16) {
    let mut client = WatcherClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    );

    let mut stream = client
        .watch(msgs::WatchRequest { id: 1 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    stream.next().await.expect("one item").expect("ok item");
    drop(stream);
}

async fn saw_cancel_within(flag: &AtomicBool, limit: Duration) -> bool {
    for _ in 0..(limit.as_millis() / 50) {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Named by the attribute, so the reply is re-typed and its producer hears the
/// caller leave.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_opted_in_stream_cancels_the_work_feeding_it() {
    OPT_IN_SAW_CANCEL.store(false, Ordering::SeqCst);

    let (port, shutdown) = boot(WithOptInModule).await;
    abandon_after_one_item(port).await;

    assert!(
        saw_cancel_within(&OPT_IN_SAW_CANCEL, Duration::from_secs(2)).await,
        "the task feeding the reply must learn the caller went away"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// The control. Neither the response type nor the method's name reaches
/// `StreamProgressStream`, so without the attribute the reply is served as the
/// handler built it — which is what the attribute is there to change.
#[serial]
#[tokio_localset_test::localset_test]
async fn the_same_service_without_the_attribute_is_not_covered() {
    CONTROL_SAW_CANCEL.store(false, Ordering::SeqCst);

    let (port, shutdown) = boot(WithoutOptInModule).await;
    abandon_after_one_item(port).await;

    assert!(
        !saw_cancel_within(&CONTROL_SAW_CANCEL, Duration::from_millis(600)).await,
        "without the attribute nothing identifies this method as streaming"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}
