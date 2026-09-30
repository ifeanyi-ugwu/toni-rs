//! Calling a gRPC service from inside a ulo application.
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
use std::sync::Arc;

use ulo::http::Body;
use ulo::{UloFactory, module, provide};
use ulo_http_axum::AxumAdapter;
use ulo_macros::{controller, get, grpc_methods, new, routes};

mod orders_pb {
    tonic::include_proto!("ulo_examples.orders");
}

use orders_pb::orders_client::OrdersClient;
use orders_pb::orders_server::{Orders, OrdersServer};
use tonic::transport::Channel;

const GRPC_ADDR: &str = "127.0.0.1:50051";

// ── the caller ─────────────────────────────────────────────────────────────

#[controller("/orders")]
pub struct OrdersGateway {
    /// Injected by type, as the one client the module declares. A clone of
    /// `OrdersClient<Channel>` shares the connection, so a handler clones it
    /// out of the `Arc` to get the `&mut` tonic wants.
    #[inject]
    orders: Arc<OrdersClient<Channel>>,
}

#[routes]
impl OrdersGateway {
    #[get("/place")]
    async fn place(&self) -> Body {
        let mut orders = (*self.orders).clone();
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

#[grpc_methods(orders_pb::orders_server::Orders)]
impl OrdersService {
    #[grpc_method]
    async fn create(
        &self,
        ulo::extract::Payload(req): ulo::extract::Payload<orders_pb::CreateOrderRequest>,
    ) -> orders_pb::CreateOrderResponse {
        orders_pb::CreateOrderResponse {
            id: 1,
            item: req.item.clone(),
            qty: req.qty,
            status: format!("created:{}", req.item),
        }
    }
}

#[module(
    controllers: [OrdersGateway, OrdersService],
    providers: [provide!(async || {
        // `connect_lazy` dials on the first call rather than here, so a peer
        // that is not up yet cannot fail startup.
        let channel = Channel::from_static("http://127.0.0.1:50051").connect_lazy();
        OrdersClient::new(channel)
    })]
)]
impl AppModule {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let grpc_addr: SocketAddr = GRPC_ADDR.parse().unwrap();
    let mut app = UloFactory::create(AppModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();
    app.use_grpc_adapter(ulo_grpc::GrpcAdapter::new(grpc_addr))
        .unwrap();

    println!("HTTP on http://127.0.0.1:3000, gRPC on {GRPC_ADDR}");
    println!("try: curl localhost:3000/orders/place");
    app.start().await.unwrap();
}
