//! Per-request tracing spans on the TCP and UDP RPC adapters.
//!
//! Both adapters wrap each handler in an `rpc.request` span carrying the
//! transport, message pattern, optional correlation id, and peer address.
//! Any tracing event emitted from the user handler — or from the adapter
//! itself (panic catcher, write errors) — automatically inherits those
//! fields, so an operator scanning logs can correlate every line back to
//! the originating message.
//!
//! Run:
//!
//! ```
//! RUST_LOG=info cargo run --example rpc_tracing
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
//! ```
//!
//! The server-side log output looks like:
//!
//! ```text
//! INFO rpc.request{transport="tcp" pattern=orders.create id=Some("req-1") peer=127.0.0.1:54321}: rpc_tracing: handler called
//! ```
//!
//! Notice the span fields (`transport`, `pattern`, `id`, `peer`) are
//! automatically attached to the `tracing::info!` call inside the handler —
//! the user handler never had to mention them.

use toni::ToniFactory;
use toni_macros::{module, rpc_controller};
use tracing_subscriber::{fmt, EnvFilter};

#[rpc_controller(pub struct OrdersController {})]
impl OrdersController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.create")]
    async fn create_order(
        &self,
        data: toni::RpcData,
        _ctx: toni::RpcContext,
    ) -> Result<toni::RpcData, toni::RpcError> {
        let payload = data
            .as_json()
            .ok_or_else(|| toni::RpcError::Internal("expected JSON payload".into()))?;

        let item = payload["item"].as_str().unwrap_or("unknown");
        let qty = payload["qty"].as_u64().unwrap_or(1);

        // This event has no explicit fields, yet operator-side it appears
        // with `transport=`, `pattern=`, `id=`, `peer=` attached — those
        // come from the surrounding span the adapter installed.
        tracing::info!(item, qty, "handler called");

        Ok(toni::RpcData::json(serde_json::json!({
            "id": 1001,
            "item": item,
            "qty": qty,
            "status": "created"
        })))
    }
}

#[module(providers: [OrdersController])]
struct OrdersModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install a subscriber that prints span fields with each event.
    // Without this the framework's tracing calls go nowhere — see the
    // `logging` example for more setups.
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,toni_tcp=debug,toni_udp=debug")),
        )
        .with_target(true)
        .init();

    println!("RPC tracing example");
    println!("TCP: 127.0.0.1:4000");
    println!("UDP: 127.0.0.1:4001");
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

    // The point of this example is to show TCP and UDP side-by-side, but
    // a single Toni app today registers one RPC adapter at a time and the
    // factory is `!Send` (it stores its container in `Rc<RefCell<…>>`),
    // so we can't `tokio::spawn` two of them. A `LocalSet` lets two apps
    // share this thread — both make progress concurrently. In a real
    // deployment you'd typically pick one transport.
    let local = tokio::task::LocalSet::new();

    local.spawn_local(async {
        let mut app = ToniFactory::new().create_with(OrdersModule).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 4000))
            .unwrap();
        app.start().await.unwrap();
    });

    local.spawn_local(async {
        let mut app = ToniFactory::new().create_with(OrdersModule).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", 4001))
            .unwrap();
        app.start().await.unwrap();
    });

    local.await;
    Ok(())
}
