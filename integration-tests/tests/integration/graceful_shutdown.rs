//! Graceful shutdown integration tests
//!
//! Verifies that `app.close()`:
//!   1. Sends close frames to connected WebSocket clients
//!   2. Stops the HTTP server from accepting new connections
//!   3. Runs the `on_module_destroy` lifecycle hook

use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use toni::module;
use toni::toni_factory::ToniFactory;
use toni::websocket::{BroadcastModule, BroadcastService, WsClient, WsHandlerResult, WsMessage};
use toni_axum::AxumAdapter;
use toni_macros::{new, on_module_destroy, subscriptions, websocket_gateway};

static DESTROY_HOOK_RAN: AtomicBool = AtomicBool::new(false);

#[websocket_gateway("/ws")]
pub struct CloseGateway {
    #[inject]
    broadcast: BroadcastService,
}
#[subscriptions]
impl CloseGateway {
    #[new]
    pub fn new(broadcast: BroadcastService) -> Self {
        Self { broadcast }
    }

    #[on_module_destroy]
    async fn on_destroy(&self) {
        DESTROY_HOOK_RAN.store(true, Ordering::SeqCst);
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [CloseGateway], imports: [BroadcastModule::new()])]
struct CloseModule;

/// Shutdown via ShutdownHandle sends WS close frames, stops HTTP, and fires on_module_destroy.
#[serial]
#[tokio_localset_test::localset_test]
async fn app_close_disconnects_ws_clients_and_stops_http() {
    DESTROY_HOOK_RAN.store(false, Ordering::SeqCst);

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(CloseModule).await;
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let addr = bound.http.expect("HTTP adapter not bound");
        let _ = addr_tx.send(addr);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let addr = addr_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    // Verify the WS gateway is reachable before shutdown.
    let ws_url = format!("ws://{}/ws", addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let pong = ws.next().await.unwrap().unwrap();
    assert_eq!(pong.to_text().unwrap(), "pong");

    // Trigger graceful shutdown and wait for completion.
    shutdown.shutdown();
    shutdown.completed().await;

    // The server must have sent a close frame (or dropped the connection).
    let next = ws.next().await;
    assert!(
        matches!(
            next,
            None | Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
        ),
        "expected WS close after shutdown, got {:?}",
        next
    );

    // HTTP server must no longer accept new connections.
    let result = reqwest::get(format!("http://{}/", addr)).await;
    assert!(
        result.is_err(),
        "HTTP server should be stopped after shutdown"
    );

    // on_module_destroy must have run.
    assert!(
        DESTROY_HOOK_RAN.load(Ordering::SeqCst),
        "on_module_destroy hook should run during shutdown"
    );
}
