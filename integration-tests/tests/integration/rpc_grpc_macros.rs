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
//! - An injected dependency reaches the handler, verifying DI works.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
}

#[module(providers: [OrdersCounter, OrdersGrpcService])]
struct GrpcMacrosModule;

#[tokio_localset_test::localset_test]
async fn grpc_service_macro_di_round_trip() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);

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

    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    let endpoint = format!("http://127.0.0.1:{}", port);
    let mut client = tonic::transport::Endpoint::from_shared(endpoint)
        .unwrap()
        .connect()
        .await
        .map(OrdersClient::new)
        .expect("gRPC connect should succeed");

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

    // The handler-side validation path also reaches the service through DI.
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
