//! Calling a gRPC service from inside a toni application.
//!
//! The client is a provider like any other: registered under its own type, so a
//! controller asks for it with `#[inject]` and never builds one itself. The
//! endpoint connects lazily, so nothing in startup depends on the peer being up.
//!
//! Here the HTTP route calls a gRPC service this same process serves, which
//! keeps the example to one command. In a real deployment the URL points at
//! another service and nothing else changes.
//!
//! Run:
//!
//! ```
//! cargo run --example grpc_client
//! ```
//!
//! Then in a second terminal:
//!
//! ```
//! curl localhost:3000/orders/place
//! # {"id":1,"status":"created:keyboard"}
//! ```

use std::net::SocketAddr;

use toni::{module, provider_factory, Body, ToniFactory};
use toni_axum::AxumAdapter;
use toni_macros::{controller, get, grpc_methods, new, routes};

mod orders_pb {
    tonic::include_proto!("toni_examples.orders");
}

use orders_pb::orders_client::OrdersClient;
use orders_pb::orders_server::{Orders, OrdersServer};
use tonic::transport::Channel;

const GRPC_ADDR: &str = "127.0.0.1:50051";

// ── the caller ─────────────────────────────────────────────────────────────

#[controller("/orders")]
pub struct OrdersGateway {
    /// Injected by type. `OrdersClient<Channel>` is cheap to clone — the
    /// clone shares the connection — so a handler clones it to get the `&mut`
    /// tonic wants.
    #[inject]
    orders: OrdersClient<Channel>,
}

#[routes]
impl OrdersGateway {
    #[get("/place")]
    async fn place(&self) -> Body {
        let mut orders = self.orders.clone();
        match orders
            .create(orders_pb::CreateOrderRequest {
                item: "keyboard".to_string(),
                qty: 1,
            })
            .await
        {
            Ok(reply) => {
                let reply = reply.into_inner();
                Body::json(serde_json::json!({
                    "id": reply.id,
                    "status": reply.status,
                }))
            }
            // The peer being down is an ordinary error here, not a startup
            // failure, because the endpoint connected lazily.
            Err(status) => Body::json(serde_json::json!({
                "error": status.code().to_string(),
                "message": status.message(),
            })),
        }
    }
}

// ── the service it calls ───────────────────────────────────────────────────

#[controller]
pub struct OrdersService {}

impl OrdersService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for OrdersService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 1,
            item: req.item.clone(),
            qty: req.qty,
            status: format!("created:{}", req.item),
        }))
    }
}

#[module(
    controllers: [OrdersGateway, OrdersService],
    providers: [provider_factory!(OrdersClient<Channel>, || {
        // `connect_lazy` dials on the first call rather than here, so a peer
        // that is not up yet cannot fail startup.
        let channel = Channel::from_static("http://127.0.0.1:50051").connect_lazy();
        OrdersClient::new(channel)
    })]
)]
impl AppModule {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let grpc_addr: SocketAddr = GRPC_ADDR.parse().unwrap();
            let mut app = ToniFactory::create(AppModule).await.unwrap();
            app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
                .unwrap();
            app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(grpc_addr))
                .unwrap();

            println!("HTTP on http://127.0.0.1:3000, gRPC on {GRPC_ADDR}");
            println!("try: curl localhost:3000/orders/place");
            app.start().await.unwrap();
        })
        .await;
}
