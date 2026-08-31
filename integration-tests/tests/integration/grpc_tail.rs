//! A gRPC streaming reply outlives the handler that returned it, and the
//! framework holds the execution open across it: the context rides the stream,
//! and a stream dropped with items still to come fires the execution's
//! cancellation token.
//!
//! The token is reachable because the context rides the request — a gRPC
//! handler's signature is the tonic trait's and cannot carry one.
//!
//! The last test covers the other legal spelling of a streaming signature.
//! `#[grpc_methods]` reads the response type where it says `Self::SomeStream`,
//! and pairs the method with its associated type by name where it does not, so
//! a service written either way is covered.

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serial_test::serial;
use toni::context::{CancellationToken, GrpcContext, HandlerContext};
use toni::ToniFactory;
use toni_macros::{controller, grpc_methods, module, new};

mod tail_pb {
    tonic::include_proto!("toni_test.orders");
}

use tail_pb::orders_client::OrdersClient;
use tail_pb::orders_server::{Orders, OrdersServer};

type EventStream =
    Pin<Box<dyn Stream<Item = Result<tail_pb::ProgressEvent, tonic::Status>> + Send>>;
type ChatEventStream =
    Pin<Box<dyn Stream<Item = Result<tail_pb::ChatMessage, tonic::Status>> + Send>>;

static SAW_CANCEL: AtomicBool = AtomicBool::new(false);
static PRODUCED: AtomicUsize = AtomicUsize::new(0);
static TOKEN: Mutex<Option<CancellationToken>> = Mutex::new(None);
static METHOD: Mutex<Option<String>> = Mutex::new(None);

/// Feeds the reply from outside the handler's future, so nothing but the token
/// can stop it: the send result is dropped, since a closed receiver would
/// otherwise be the signal and the token is what these tests pin.
fn detached_producer(context: GrpcContext, id: u64) -> EventStream {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for _ in 0..200 {
            let _ = tx.send(Ok(tail_pb::ProgressEvent {
                id,
                status: "tick".to_string(),
            }));
            PRODUCED.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                _ = context.cancellation().cancelled() => {
                    SAW_CANCEL.store(true, Ordering::SeqCst);
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    });
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

fn finite_stream() -> EventStream {
    Box::pin(futures_util::stream::iter((0..3).map(|n| {
        Ok(tail_pb::ProgressEvent {
            id: 0,
            status: format!("step-{}", n),
        })
    })))
}

// ── the covered service: response types written `Self::…` ──────────────────

#[controller]
pub struct TailGrpcService {}

impl TailGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for TailGrpcService {
    async fn create(
        &self,
        request: tonic::Request<tail_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<tail_pb::CreateOrderResponse>, tonic::Status> {
        let context =
            GrpcContext::of(request.extensions()).expect("dispatched through the framework");
        *METHOD.lock().unwrap() = Some(context.method().to_string());
        Ok(tonic::Response::new(tail_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        }))
    }

    type WatchProgressStream = EventStream;

    /// `id == 0` answers a stream that ends; anything else never does, so a drop
    /// of it can only be the caller having gone.
    async fn watch_progress(
        &self,
        request: tonic::Request<tail_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let context =
            GrpcContext::of(request.extensions()).expect("dispatched through the framework");
        *TOKEN.lock().unwrap() = Some(context.cancellation().clone());

        let id = request.into_inner().id;
        if id == 0 {
            return Ok(tonic::Response::new(finite_stream()));
        }
        Ok(tonic::Response::new(detached_producer(context, id)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<tail_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<tail_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream = ChatEventStream;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<tail_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [TailGrpcService])]
impl TailGrpcModule {}

// ── the same reply, spelled without `Self::` ───────────────────────────────

#[controller]
pub struct ConcreteGrpcService {}

impl ConcreteGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for ConcreteGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<tail_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<tail_pb::CreateOrderResponse>, tonic::Status> {
        Ok(tonic::Response::new(tail_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        }))
    }

    type WatchProgressStream = EventStream;

    /// `EventStream`, not `Self::WatchProgressStream` — legal, and the same type
    /// after normalisation. The macro reaches it through the method's name.
    async fn watch_progress(
        &self,
        request: tonic::Request<tail_pb::WatchRequest>,
    ) -> Result<tonic::Response<EventStream>, tonic::Status> {
        let context =
            GrpcContext::of(request.extensions()).expect("dispatched through the framework");
        *TOKEN.lock().unwrap() = Some(context.cancellation().clone());
        let id = request.into_inner().id;
        Ok(tonic::Response::new(detached_producer(context, id)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<tail_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<tail_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream = ChatEventStream;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<tail_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [ConcreteGrpcService])]
impl ConcreteGrpcModule {}

// ── harness ────────────────────────────────────────────────────────────────

async fn boot<M>(module: M) -> (u16, toni::ShutdownHandle)
where
    M: toni::ModuleMetadata + 'static,
{
    boot_with(module, |a| a).await
}

async fn boot_with<M, F>(module: M, configure: F) -> (u16, toni::ShutdownHandle)
where
    M: toni::ModuleMetadata + 'static,
    F: FnOnce(toni_grpc::GrpcAdapter) -> toni_grpc::GrpcAdapter + Send + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = configure(toni_grpc::GrpcAdapter::new(addr));
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

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    )
}

async fn saw_cancel_within(limit: Duration) -> bool {
    let deadline = limit.as_millis() / 50;
    for _ in 0..deadline {
        if SAW_CANCEL.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

// ── tests ──────────────────────────────────────────────────────────────────

/// The producer stops when the caller goes, rather than at whatever it was
/// going to do next.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_abandoned_grpc_stream_cancels_the_work_feeding_it() {
    SAW_CANCEL.store(false, Ordering::SeqCst);
    PRODUCED.store(0, Ordering::SeqCst);

    let (port, shutdown) = boot(TailGrpcModule).await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(tail_pb::WatchRequest { id: 7 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    stream.next().await.expect("one item").expect("ok item");
    assert!(
        !SAW_CANCEL.load(Ordering::SeqCst),
        "a stream still being read must not read as abandoned"
    );

    drop(stream);

    assert!(
        saw_cancel_within(Duration::from_secs(2)).await,
        "the task feeding the reply must learn the caller went away"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// The other half, and what makes the first mean anything: a reply read to its
/// last item is not an abandoned one.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_drained_grpc_stream_is_not_cancelled() {
    *TOKEN.lock().unwrap() = None;

    let (port, shutdown) = boot(TailGrpcModule).await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(tail_pb::WatchRequest { id: 0 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    let mut seen = Vec::new();
    while let Some(item) = stream.next().await {
        seen.push(item.expect("ok item").status);
    }
    assert_eq!(seen, vec!["step-0", "step-1", "step-2"]);
    drop(stream);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let token = TOKEN
        .lock()
        .unwrap()
        .clone()
        .expect("the handler published its token");
    assert!(
        !token.is_cancelled(),
        "a drained reply is not an abandoned one"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// The request is the only place a gRPC handler can take its context from, and
/// what it finds there names the call the way the wire does — with the package,
/// and with the route's own casing rather than the Rust method name's.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_reads_its_context_off_the_request() {
    *METHOD.lock().unwrap() = None;

    let (port, shutdown) = boot(TailGrpcModule).await;
    let mut client = connect(port).await;

    client
        .create(tail_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        })
        .await
        .expect("unary call must succeed");

    assert_eq!(
        METHOD.lock().unwrap().as_deref(),
        Some("toni_test.orders.Orders/Create"),
        "the handler must see the path the caller dialled, not the Rust names"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// The other legal spelling of the same signature. Nothing about the service
/// differs but the tokens its return type is written with, and the reply is
/// covered the same way.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_concrete_stream_signature_is_covered_too() {
    SAW_CANCEL.store(false, Ordering::SeqCst);
    PRODUCED.store(0, Ordering::SeqCst);

    let (port, shutdown) = boot(ConcreteGrpcModule).await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(tail_pb::WatchRequest { id: 11 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    stream.next().await.expect("one item").expect("ok item");
    assert!(!SAW_CANCEL.load(Ordering::SeqCst));

    drop(stream);

    assert!(
        saw_cancel_within(Duration::from_secs(2)).await,
        "a reply named without `Self::` is the same reply"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// Shutdown with a reply still in flight. The deadline ends it rather than
/// abandoning it: tonic serves each connection from a detached task, so a reply
/// that never finishes would otherwise outlive the shutdown that reported
/// itself complete.
#[serial]
#[tokio_localset_test::localset_test]
async fn the_drain_deadline_ends_a_reply_it_cannot_wait_for() {
    SAW_CANCEL.store(false, Ordering::SeqCst);
    PRODUCED.store(0, Ordering::SeqCst);

    let (port, shutdown) = boot_with(TailGrpcModule, |a| {
        a.with_drain_timeout(Duration::from_millis(300))
    })
    .await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(tail_pb::WatchRequest { id: 7 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    stream.next().await.expect("one item").expect("ok item");

    shutdown.shutdown();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), shutdown.completed())
            .await
            .is_ok(),
        "shutdown must complete rather than report complete while still serving"
    );

    // The caller is told why, rather than losing the connection under it.
    let ending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Ok(_)) => continue,
                other => return other,
            }
        }
    })
    .await
    .expect("the reply must end");

    match ending {
        Some(Err(status)) => assert_eq!(status.code(), tonic::Code::Unavailable),
        other => panic!(
            "expected UNAVAILABLE, got {:?}",
            other.map(|r| r.map(|_| ()))
        ),
    }

    assert!(
        saw_cancel_within(Duration::from_secs(2)).await,
        "ending the reply must reach the task feeding it"
    );
}
