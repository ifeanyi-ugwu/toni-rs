// RPC controller example using the NATS transport adapter.
//
// Requires a running NATS server: `nats-server` or `docker run -p 4222:4222 nats`
//
// The NATS subject is the handler pattern — no envelope wrapper needed.
// Request-response uses a NATS reply-to inbox set by the caller; fire-and-forget
// messages have no reply-to and produce no response on the wire.
//
// Test with the NATS CLI (https://github.com/nats-io/natscli):
//
//   # request-response — handler returns Ok(data)
//   nats req order.create '{"item":"keyboard","qty":3}'
//   → {"response":{"id":1001,"item":"keyboard","qty":3,"status":"created"}}
//
//   # error — handler returns Err
//   nats req order.create '{"item":"keyboard","qty":0}'
//   → {"response":{"status":"error","kind":"Internal","message":"qty must be positive"}}
//
//   # fire-and-forget — no reply-to, no response
//   nats pub order.shipped '{"order_id":1001}'

use std::sync::Arc;
use ulo::UloFactory;
use ulo_macros::{controller, injectable, module, new, patterns};

// ============================================================================
// Service
// ============================================================================

#[injectable]
pub struct OrdersService {}
impl OrdersService {
    pub fn create_order(&self, item: &str, qty: u32) -> serde_json::Value {
        println!("  [OrdersService] Creating order: {} x{}", item, qty);
        serde_json::json!({ "id": 1001, "item": item, "qty": qty, "status": "created" })
    }

    pub fn handle_shipment(&self, order_id: u64) {
        println!("  [OrdersService] Order {} marked as shipped", order_id);
    }
}

// ============================================================================
// RPC controller
// ============================================================================

#[controller]
pub struct OrdersController {
    #[inject]
    service: Arc<OrdersService>,
}
#[patterns]
impl OrdersController {
    #[new]
    pub fn new(service: Arc<OrdersService>) -> Self {
        Self { service }
    }

    #[message_pattern("order.create")]
    async fn create_order(
        &self,
        data: ulo::rpc::RpcData,
        _ctx: &ulo::rpc::RpcContext,
    ) -> Result<ulo::rpc::RpcData, ulo::rpc::RpcError> {
        let payload = data
            .as_json()
            .ok_or_else(|| ulo::rpc::RpcError::Internal("expected JSON payload".into()))?;

        let item = payload["item"].as_str().unwrap_or("unknown");
        let qty = payload["qty"].as_u64().unwrap_or(1) as u32;

        if qty == 0 {
            return Err(ulo::rpc::RpcError::Internal("qty must be positive".into()));
        }

        let order = self.service.create_order(item, qty);
        Ok(ulo::rpc::RpcData::json(order))
    }

    #[event_pattern("order.shipped")]
    async fn on_order_shipped(
        &self,
        data: ulo::rpc::RpcData,
        _ctx: &ulo::rpc::RpcContext,
    ) -> Result<(), ulo::rpc::RpcError> {
        let payload = data
            .as_json()
            .ok_or_else(|| ulo::rpc::RpcError::Internal("expected JSON payload".into()))?;

        let order_id = payload["order_id"].as_u64().unwrap_or(0);
        self.service.handle_shipment(order_id);
        Ok(())
    }
}

// ============================================================================
// Module
// ============================================================================

#[module(controllers: [OrdersController], providers: [OrdersService])]
struct OrdersModule;

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("RPC NATS Example");
    println!("HTTP:         http://127.0.0.1:8080");
    println!("NATS subjects: order.create, order.shipped\n");

    let mut app = UloFactory::new().create_with(OrdersModule).await?;

    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 8080))
        .unwrap();
    app.use_rpc_adapter(ulo_rpc_nats::NatsAdapter::new("nats://127.0.0.1:4222"))
        .unwrap();

    app.start().await?;
    Ok(())
}
