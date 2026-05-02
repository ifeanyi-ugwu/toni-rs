//! End-to-end coverage for the `#[grpc_service]` + `#[grpc_methods]` macros:
//!
//! - `#[grpc_service]` on a struct + its inherent impl makes it an injectable
//!   DI provider that the framework discovers as a gRPC service.
//! - `#[grpc_methods]` on the proto-trait impl emits a `GrpcServiceTrait`
//!   that wraps `self` in the inferred `*Server` and registers it with
//!   the framework's gRPC adapter at bind time.
//! - The user never types `*Server::new(handler)` and never calls
//!   `adapter.add_service()` — DI + module registration is the entire
//!   wiring story.
//! - All four call modes (unary, server-streaming, client-streaming, bidi)
//!   work through the macros without per-mode special handling — the macro
//!   just hands tonic an instance of the trait impl, and tonic dispatches.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use toni::ToniFactory;
use toni_macros::{grpc_methods, grpc_service, injectable, module};

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

use orders_pb::orders_client::OrdersClient;
use orders_pb::orders_server::{Orders, OrdersServer};

#[injectable(pub struct OrdersCounter {
    seq: Arc<AtomicU64>,
})]
impl OrdersCounter {
    pub fn new() -> Self {
        Self {
            seq: Arc::new(AtomicU64::new(1000)),
        }
    }

    fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }
}

#[grpc_service(pub struct OrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl OrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self {
        Self { counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for OrdersGrpcService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        if req.qty == 0 {
            return Err(tonic::Status::invalid_argument("qty must be positive"));
        }
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let id = request.into_inner().id;
        let stream = futures_util::stream::iter(
            ["queued", "picked", "shipped"]
                .into_iter()
                .map(move |status| {
                    Ok(orders_pb::ProgressEvent {
                        id,
                        status: status.to_string(),
                    })
                }),
        );
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        let mut stream = request.into_inner();
        let mut created: u32 = 0;
        let first_id = self.counter.next_id();
        // Reserve subsequent ids contiguously so the response can summarise.
        while let Some(item) = stream.next().await {
            let _req = item?;
            if created > 0 {
                self.counter.next_id();
            }
            created += 1;
        }
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created,
            first_id,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let mut inbound = request.into_inner();
        let counter = self.counter.clone();
        let outbound = async_stream::stream! {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(m) => yield Ok(orders_pb::ChatMessage {
                        text: m.text,
                        id: counter.next_id(),
                    }),
                    Err(e) => yield Err(e),
                }
            }
        };
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, OrdersGrpcService])]
struct GrpcMacrosModule;

/// Boots the gRPC server with the default drain timeout.
async fn boot() -> (u16, toni::ShutdownHandle) {
    boot_with(|a| a).await
}

/// Boots the gRPC server, applying a custom configuration to the adapter
/// before it's registered (e.g. `with_drain_timeout`).
async fn boot_with<F>(configure: F) -> (u16, toni::ShutdownHandle)
where
    F: FnOnce(toni_grpc::GrpcAdapter) -> toni_grpc::GrpcAdapter + Send + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = configure(toni_grpc::GrpcAdapter::new(addr));
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(GrpcMacrosModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound
            .grpc
            .expect("BoundAdapters.grpc must be populated")
            .port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    let endpoint = format!("http://127.0.0.1:{}", port);
    tonic::transport::Endpoint::from_shared(endpoint)
        .unwrap()
        .connect()
        .await
        .map(OrdersClient::new)
        .expect("gRPC connect should succeed")
}

#[tokio_localset_test::localset_test]
async fn grpc_service_macro_di_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let resp = tokio::time::timeout(
        Duration::from_secs(2),
        client.create(orders_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 3,
        }),
    )
    .await
    .expect("call must reply within 2s")
    .expect("call must succeed")
    .into_inner();

    assert!(resp.id >= 1000, "id should come from the injected counter, got {}", resp.id);
    assert_eq!(resp.status, "created:keyboard");

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".to_string(),
            qty: 0,
        })
        .await
        .expect_err("qty=0 must fail");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

#[tokio_localset_test::localset_test]
async fn grpc_server_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(orders_pb::WatchRequest { id: 42 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    let mut statuses = Vec::new();
    while let Some(item) = stream.next().await {
        let evt = item.expect("stream item must be Ok");
        assert_eq!(evt.id, 42, "server-streaming events must echo the request id");
        statuses.push(evt.status);
    }
    assert_eq!(statuses, vec!["queued", "picked", "shipped"]);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

#[tokio_localset_test::localset_test]
async fn grpc_client_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let outbound = futures_util::stream::iter(vec![
        orders_pb::CreateOrderRequest { item: "a".into(), qty: 1 },
        orders_pb::CreateOrderRequest { item: "b".into(), qty: 2 },
        orders_pb::CreateOrderRequest { item: "c".into(), qty: 3 },
    ]);

    let resp = client
        .bulk_create(outbound)
        .await
        .expect("client-streaming call must succeed")
        .into_inner();

    assert_eq!(resp.created, 3);
    assert!(
        resp.first_id >= 1000,
        "first_id should come from the injected counter, got {}",
        resp.first_id
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// Without drain enforcement a never-closing bidi stream pins the server
/// open after `shutdown()` because tonic's `serve_with_incoming_shutdown`
/// waits for in-flight handlers to return. With `with_drain_timeout` the
/// budget elapses, the serve future is dropped, and the in-flight stream
/// is aborted (clients see UNAVAILABLE).
#[tokio_localset_test::localset_test]
async fn grpc_drain_timeout_aborts_long_running_streams() {
    let drain = Duration::from_millis(150);
    let (port, shutdown) = boot_with(move |a| a.with_drain_timeout(drain)).await;
    let mut client = connect(port).await;

    // Open a bidi stream where the client never closes its outbound channel
    // — the server-side `chat()` handler stays parked in `inbound.next().await`.
    let (tx, rx) = tokio::sync::mpsc::channel::<orders_pb::ChatMessage>(1);
    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut stream = client
        .chat(outbound)
        .await
        .expect("bidi call must succeed")
        .into_inner();

    // Send one message so the server enters the handler and starts blocking.
    tx.send(orders_pb::ChatMessage {
        text: "ping".into(),
        id: 0,
    })
    .await
    .unwrap();
    // Wait for the server's reply so we know the handler is mid-flight.
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("first echo should arrive")
        .expect("stream item present")
        .expect("item is Ok");

    // Trigger shutdown. Without enforcement, completed() would hang because
    // the bidi stream is still in-flight from the server's perspective.
    let before = tokio::time::Instant::now();
    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(1), shutdown.completed())
        .await
        .expect("drain budget should bound shutdown");
    let elapsed = before.elapsed();
    assert!(
        elapsed >= drain,
        "shutdown raced past the drain budget — was the timer skipped? elapsed={:?}",
        elapsed,
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "shutdown took noticeably longer than the drain budget — was it enforced? elapsed={:?}",
        elapsed,
    );
}

#[tokio_localset_test::localset_test]
async fn grpc_bidi_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let outbound = futures_util::stream::iter(vec![
        orders_pb::ChatMessage { text: "hello".into(), id: 0 },
        orders_pb::ChatMessage { text: "world".into(), id: 0 },
    ]);

    let mut inbound = client
        .chat(outbound)
        .await
        .expect("bidi call must succeed")
        .into_inner();

    let mut texts = Vec::new();
    let mut ids = Vec::new();
    while let Some(item) = inbound.next().await {
        let m = item.expect("bidi item must be Ok");
        texts.push(m.text);
        ids.push(m.id);
    }
    assert_eq!(texts, vec!["hello", "world"]);
    assert_eq!(ids.len(), 2);
    assert!(ids[0] >= 1000 && ids[1] == ids[0] + 1, "ids must come from the counter");

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}
