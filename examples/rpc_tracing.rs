//! Per-request tracing spans on the TCP, UDP, and gRPC RPC adapters.
//!
//! All three adapters wrap each handler in an `rpc.request` span carrying
//! the transport, message pattern (or service/method for gRPC), optional
//! correlation id, and peer address. Any tracing event emitted from the
//! user handler — or from the adapter itself (panic catcher, write
//! errors) — automatically inherits those fields, so an operator scanning
//! logs can correlate every line back to the originating message.
//!
//! Run:
//!
//! ```
//! cargo run --example rpc_tracing
//! ```
//!
//! Then in a second terminal:
//!
//! ```
//! # TCP — request-response
//! echo '{"pattern":"orders.create","data":{"item":"keyboard","qty":3},"id":"req-1"}' \
//!   | nc 127.0.0.1 4000
//!
//! # UDP — request-response
//! echo '{"pattern":"orders.create","data":{"item":"mouse","qty":1},"id":"req-2"}' \
//!   | nc -u -w1 127.0.0.1 4001
//!
//! # gRPC — using grpcurl (requires the proto file in this example's tree)
//! grpcurl -plaintext -d '{"item":"monitor","qty":2}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     -H 'x-request-id: req-3' \
//!     127.0.0.1:5000 toni_examples.orders.Orders/Create
//! ```
//!
//! The server-side log output looks like:
//!
//! ```text
//! INFO rpc.request{transport="tcp" pattern=orders.create id=Some("req-1") peer=127.0.0.1:54321}: rpc_tracing: handler called item=keyboard qty=3
//! INFO rpc.request{transport="grpc" pattern=toni_examples.orders.Orders/Create id=Some("req-3") peer=127.0.0.1:54322}: rpc_tracing: grpc handler called item=monitor qty=2
//! ```
//!
//! The span fields (`transport`, `pattern`, `id`, `peer`) are automatically
//! attached to `tracing::info!` calls inside the handler — the user
//! handler never had to mention them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use toni::ToniFactory;
use toni_macros::{grpc_methods, grpc_service, injectable, module, new, patterns, rpc_controller};

mod orders_pb {
    tonic::include_proto!("toni_examples.orders");
}

use orders_pb::orders_server::{Orders, OrdersServer};

// ─── TCP / UDP — pattern-based ──────────────────────────────────────────────

#[rpc_controller]
pub struct OrdersController {}
#[patterns]
impl OrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.create")]
    async fn create_order(
        &self,
        data: toni::RpcData,
        _ctx: &toni::context::RpcContext,
    ) -> Result<toni::RpcData, toni::RpcError> {
        let payload = data
            .as_json()
            .ok_or_else(|| toni::RpcError::Internal("expected JSON payload".into()))?;

        let item = payload["item"].as_str().unwrap_or("unknown");
        let qty = payload["qty"].as_u64().unwrap_or(1);

        // Span fields (`transport`, `pattern`, `id`, `peer`) attach
        // automatically — we never mention them here.
        tracing::info!(item, qty, "handler called");

        Ok(toni::RpcData::json(serde_json::json!({
            "id": 1001,
            "item": item,
            "qty": qty,
            "status": "created"
        })))
    }
}

#[module(controllers: [OrdersController])]
struct PatternModule;

// ─── gRPC — contract-first ──────────────────────────────────────────────────

#[injectable]
pub struct OrdersCounter {
    seq: Arc<AtomicU64>,
}
impl OrdersCounter {
    #[new]
    pub fn new() -> Self {
        Self {
            seq: Arc::new(AtomicU64::new(2000)),
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

        // Same span shape as the TCP/UDP handler above — the only thing
        // that changes operator-side is `transport="grpc"`.
        tracing::info!(item = %req.item, qty = req.qty, "grpc handler called");

        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            item: req.item,
            qty: req.qty,
            status: "created".to_string(),
        }))
    }
}

#[module(controllers: [OrdersGrpcService], providers: [OrdersCounter])]
struct GrpcModule;

// ─── Entry point ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("RPC tracing example");
    println!("TCP:  127.0.0.1:4000");
    println!("UDP:  127.0.0.1:4001");
    println!("gRPC: 127.0.0.1:5000");
    println!();
    println!("Send a request from another terminal:");
    println!();
    println!("  # TCP");
    println!(
        r#"  echo '{{"pattern":"orders.create","data":{{"item":"keyboard","qty":3}},"id":"req-1"}}' \"#
    );
    println!("    | nc 127.0.0.1 4000");
    println!();
    println!("  # UDP");
    println!(
        r#"  echo '{{"pattern":"orders.create","data":{{"item":"mouse","qty":1}},"id":"req-2"}}' \"#
    );
    println!("    | nc -u -w1 127.0.0.1 4001");
    println!();
    println!("  # gRPC (requires `grpcurl`)");
    println!(r#"  grpcurl -plaintext -d '{{"item":"monitor","qty":2}}' \"#);
    println!(r#"      -proto examples/proto/orders.proto \"#);
    println!(r#"      -import-path examples/proto \"#);
    println!(r#"      -H 'x-request-id: req-3' \"#);
    println!("      127.0.0.1:5000 toni_examples.orders.Orders/Create");
    println!();

    // ToniFactory is `!Send`. A `LocalSet` lets all three apps share this
    // thread; in a real deployment you'd typically pick one transport.
    let local = tokio::task::LocalSet::new();

    local.spawn_local(async {
        let mut app = ToniFactory::new().create_with(PatternModule).await.unwrap();
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 4000))
            .unwrap();
        app.start().await.unwrap();
    });

    local.spawn_local(async {
        let mut app = ToniFactory::new().create_with(PatternModule).await.unwrap();
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", 4001))
            .unwrap();
        app.start().await.unwrap();
    });

    local.spawn_local(async {
        let addr: std::net::SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let mut app = ToniFactory::new().create_with(GrpcModule).await.unwrap();
        app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(addr))
            .unwrap();
        app.start().await.unwrap();
    });

    local.await;
    Ok(())
}
