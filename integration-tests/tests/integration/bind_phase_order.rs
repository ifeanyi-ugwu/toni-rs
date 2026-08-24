//! `bind` asks every adapter to take what the application declares before it asks any of them for
//! a socket.
//!
//! The observable consequence is that a declaration which cannot work fails with nothing acquired,
//! so the teardown path only ever handles sockets. Interleaved, a separate-port WebSocket gateway
//! would already be listening by the time the RPC adapter is asked for its patterns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    async_trait, module, RpcAdapter, RpcLifecycleHandle, RpcMessageCallbacks, StartupError,
    ToniFactory,
};
use toni_macros::{
    message_pattern, new, patterns, rpc_controller, subscribe_message, subscriptions,
    websocket_gateway,
};

/// Never bound by anything else; the gateway's own socket is what this test watches for.
const GATEWAY_PORT: u16 = 19420;

#[websocket_gateway("/events", port = 19420)]
pub struct EventsGateway {}

#[subscriptions]
impl EventsGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[rpc_controller]
pub struct OrdersController {}

#[patterns]
impl OrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.get")]
    async fn get(&self, data: RpcData, _ctx: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(providers: [EventsGateway], controllers: [OrdersController])]
struct BothTransportsModule;

/// Refuses its patterns, and records whether the gateway's socket existed when it was asked.
struct RefusingRpcAdapter {
    gateway_socket_was_taken: Arc<AtomicBool>,
}

#[async_trait]
impl RpcAdapter for RefusingRpcAdapter {
    fn register_handlers(
        &mut self,
        _patterns: &[String],
        _callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<()> {
        // Binding the gateway's port succeeds only while nothing else holds it.
        let taken = std::net::TcpListener::bind(("0.0.0.0", GATEWAY_PORT)).is_err();
        self.gateway_socket_was_taken.store(taken, Ordering::SeqCst);
        Err(anyhow!("this adapter refuses to take patterns"))
    }

    async fn into_lifecycle(self: Box<Self>) -> Result<RpcLifecycleHandle> {
        unreachable!("registration refused, so the socket is never asked for")
    }
}

#[tokio_localset_test::localset_test]
async fn a_refused_registration_fails_before_any_socket_is_taken() {
    let seen = Arc::new(AtomicBool::new(false));

    let mut app = ToniFactory::create(BothTransportsModule)
        .await
        .expect("the module graph is sound");
    app.use_websocket_adapter(toni_tungstenite::TungsteniteAdapter::new())
        .unwrap();
    app.use_rpc_adapter(RefusingRpcAdapter {
        gateway_socket_was_taken: seen.clone(),
    })
    .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("an adapter that refuses its patterns must fail bind");

    assert!(
        matches!(&err, StartupError::Adapter { transport, .. } if *transport == "rpc"),
        "expected an rpc adapter failure, got: {err}"
    );
    assert!(
        !seen.load(Ordering::SeqCst),
        "the gateway's socket was already bound when the RPC adapter was asked for its patterns: \
         registration must run before acquisition"
    );
    // And nothing is left holding it either way.
    std::net::TcpListener::bind(("127.0.0.1", GATEWAY_PORT))
        .expect("no socket should be held after a failed bind");
}
