//! A gRPC client is a provider like any other.
//!
//! toni has no client module for any transport — an `RpcClient` is constructed
//! in a provider too. What this pins is that the ordinary DI path carries a
//! tonic-generated client with nothing framework-side added: registered under
//! its own type, injected with a bare `#[inject]`, and connected lazily so
//! startup does not depend on the peer.

#![allow(dead_code)]

use std::pin::Pin;
use std::time::Duration;

use futures_util::Stream;
use toni::{module, provider_factory, ToniFactory};
use toni_macros::{controller, get, grpc_methods, new, routes};

mod probe_pb {
    tonic::include_proto!("toni_test.orders");
}

use probe_pb::orders_client::OrdersClient;
use probe_pb::orders_server::{Orders, OrdersServer};

// ── the server half, so the client has something real to call ──────────────

#[controller]
pub struct ProbeOrders {}

impl ProbeOrders {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for ProbeOrders {
    async fn create(
        &self,
        request: tonic::Request<probe_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<probe_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(probe_pb::CreateOrderResponse {
            id: 7,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<probe_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<probe_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("probe"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<probe_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<probe_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("probe"))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<probe_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<probe_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("probe"))
    }
}

#[module(controllers: [ProbeOrders])]
impl ProbeServerModule {}

// ── the client half: injected, not constructed in the handler ──────────────

#[controller("/probe")]
pub struct CallerController {
    #[inject]
    orders: OrdersClient<tonic::transport::Channel>,
}

#[routes]
impl CallerController {
    #[get("/place")]
    async fn place(&self) -> toni::Body {
        let mut orders = self.orders.clone();
        let reply = orders
            .create(probe_pb::CreateOrderRequest {
                item: "keyboard".to_string(),
                qty: 1,
            })
            .await
            .expect("the injected client must reach the server");
        toni::Body::text(reply.into_inner().status)
    }
}

static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

#[module(
    controllers: [CallerController],
    providers: [provider_factory!(OrdersClient<tonic::transport::Channel>, || {
        let port = PORT.load(std::sync::atomic::Ordering::SeqCst);
        // Lazy: no I/O in the factory, so DI construction cannot block on a peer.
        let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect_lazy();
        OrdersClient::new(channel)
    })]
)]
impl ProbeClientModule {}

#[serial_test::serial]
#[tokio_localset_test::localset_test]
async fn an_injected_tonic_client_reaches_a_toni_server() {
    // Server first, so the port is known before the client factory runs.
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ProbeServerModule).await.unwrap();
        app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(addr))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    PORT.store(port_rx.await.unwrap(), std::sync::atomic::Ordering::SeqCst);

    let server = crate::common::TestServer::start(ProbeClientModule).await;
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        server.client().get(server.url("/probe/place")).send(),
    )
    .await
    .expect("the HTTP call must return")
    .unwrap()
    .text()
    .await
    .unwrap();

    assert_eq!(body, "created:keyboard");
}
