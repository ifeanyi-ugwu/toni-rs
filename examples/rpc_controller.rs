// RPC controller example using the TCP transport adapter.
//
// This example demonstrates:
// 1. Defining an RPC controller with #[controller]
// 2. Request-response handlers via #[message_pattern]
// 3. Fire-and-forget handlers via #[event_pattern]
// 4. Returning errors — the adapter sends an error envelope back to the caller
// 5. Automatic DI wiring — the controller is discovered from the module
// 6. Wiring the TcpAdapter as the RPC transport
//
// Wire protocol: newline-delimited JSON over TCP on port 4000.
//
// Test with netcat:
//
//   # success — handler returns Ok(data)
//   echo '{"pattern":"order.create","data":{"item":"keyboard","qty":3},"id":"req-1"}' | nc 127.0.0.1 4000
//   → {"id":"req-1","response":{"id":1001,"item":"keyboard","qty":3,"status":"created"}}
//
//   # error — the handler returned Err, so the call reached this controller and answers
//   #         in the response lane, carrying the canonical envelope
//   echo '{"pattern":"order.create","data":{"item":"keyboard","qty":0},"id":"req-2"}' | nc 127.0.0.1 4000
//   → {"id":"req-2","response":{"status":"error","kind":"Internal","message":"qty must be positive"}}
//
//   # unknown pattern — nothing routed, so the reply is an `err` frame instead
//   echo '{"pattern":"does.not.exist","data":{},"id":"req-3"}' | nc 127.0.0.1 4000
//   → {"id":"req-3","err":{"message":"Pattern not found: does.not.exist","status":"NotFound"}}
//
//   # fire-and-forget (no id → no reply regardless of outcome)
//   echo '{"pattern":"order.shipped","data":{"order_id":1001}}' | nc 127.0.0.1 4000

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
        println!("[OrdersService] Creating order: {} x{}", item, qty);
        serde_json::json!({ "id": 1001, "item": item, "qty": qty, "status": "created" })
    }

    pub fn handle_shipment(&self, order_id: u64) {
        println!("[OrdersService] Order {} marked as shipped", order_id);
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
    println!("RPC Controller Example");
    println!("HTTP: http://127.0.0.1:8080");
    println!("RPC (TCP): 127.0.0.1:4000\n");

    let mut app = UloFactory::new().create_with(OrdersModule).await?;

    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 8080))
        .unwrap();
    app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("0.0.0.0", 4000))
        .unwrap();

    app.start().await?;
    Ok(())
}
