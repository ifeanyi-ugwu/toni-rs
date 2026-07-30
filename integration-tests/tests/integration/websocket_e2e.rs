//! WebSocket end-to-end integration tests
//!
//! Exercises the full path a real client message travels:
//!
//!   WS client → HTTP upgrade → AxumWsSocket → GatewayWrapper → handler → WS client
//!
//! Four tests:
//!
//! 1. **Echo** — simple gateway, no `BroadcastModule`. Verifies the `handle_connection()`
//!    (simple) path end-to-end with a real TCP connection.
//!
//! 2. **Broadcast** — `BroadcastModule` imported, two clients. Verifies the
//!    `handle_connection_with_broadcast()` path: a message sent by one client is
//!    received by both via `BroadcastService::to_all()`.
//!
//!    Race-free via handshake: each client sends `{"event":"ping"}` and waits for `"pong"`
//!    before the broadcast is sent. Receiving `"pong"` proves the client has passed
//!    `complete_connect()` and is registered in `ConnectionManager`.
//!
//! 3. **Separate-port** — gateway declares `port = 19001` in the macro. Verifies the
//!    full separate-port path: `get_port()` routes the gateway to `TungsteniteAdapter`
//!    rather than the HTTP adapter, and a client connecting to port 19001 gets its
//!    message handled correctly.
//!
//! 4. **Separate-port shutdown** — `app.close()` stops the tungstenite server on port 19001.
//!    Verifies that the WS port refuses new connections after shutdown.

use crate::common::TestServer;
use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use std::sync::atomic::{AtomicBool, Ordering};
use toni::toni_factory::ToniFactory;
use toni::websocket::{
    BroadcastModule, BroadcastService, WsClient, WsError, WsHandlerOutput, WsHandlerResult,
    WsMessage,
};

use toni::{controller, module, post, routes, Body as ToniBody};
use toni_axum::AxumAdapter;
use toni_macros::{new, on_connect, subscriptions, websocket_gateway};
use toni_tungstenite::TungsteniteAdapter;

// ─────────────────────────────────────────────────────────────────────────────
// Echo gateway — simple request-response, no BroadcastModule
// ─────────────────────────────────────────────────────────────────────────────

#[websocket_gateway("/echo")]
pub struct EchoGateway {}
#[subscriptions]
impl EchoGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("message")]
    async fn on_message(&self, _client: WsClient, msg: WsMessage) -> WsHandlerResult {
        let text = msg
            .as_text()
            .ok_or_else(|| WsError::InvalidMessage("Expected text".into()))?;
        Ok(WsMessage::text(format!("Echo: {}", text)).into())
    }
}

#[module(providers: [EchoGateway])]
struct EchoModule;

// ─────────────────────────────────────────────────────────────────────────────
// Bare gateway — no #[subscriptions] impl at all
// ─────────────────────────────────────────────────────────────────────────────

// A `#[websocket_gateway]` with no `#[subscriptions]` impl: every behavior method resolves to its
// `WsHandlersBridge` default, so this is a complete connection-only gateway (accepts connections,
// routes nothing). The absence of an impl block is the point of the test.
#[websocket_gateway("/bare")]
pub struct BareGateway {}

#[module(providers: [BareGateway])]
struct BareModule;

// ─────────────────────────────────────────────────────────────────────────────
// Hook-only gateway — #[on_connect] with no #[subscriptions] impl
// ─────────────────────────────────────────────────────────────────────────────

static ON_CONNECT_FIRED: AtomicBool = AtomicBool::new(false);

// `#[on_connect]` is its own macro (it emits the `__toni_ws_on_connect` bridge fn), so a connection
// hook stands alone — this gateway has no `#[subscriptions]` impl and routes no messages.
#[websocket_gateway("/hook-only")]
pub struct HookOnlyGateway {}

impl HookOnlyGateway {
    #[on_connect]
    async fn connected(&self, _client: &WsClient) -> Result<(), WsError> {
        ON_CONNECT_FIRED.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[module(providers: [HookOnlyGateway])]
struct HookOnlyModule;

// ─────────────────────────────────────────────────────────────────────────────
// Room gateway — broadcast-aware, BroadcastModule required
// ─────────────────────────────────────────────────────────────────────────────

#[websocket_gateway("/room")]
pub struct RoomGateway {
    #[inject]
    broadcast: BroadcastService,
}
#[subscriptions]
impl RoomGateway {
    #[new]
    pub fn new(broadcast: BroadcastService) -> Self {
        Self { broadcast }
    }

    /// Handshake: proves the client is fully registered in ConnectionManager.
    /// Response routes back to sender only (via CM.send_to_clients in broadcast mode).
    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }

    /// Broadcast the raw message text to every connected client.
    #[subscribe_message("shout")]
    async fn on_shout(&self, _client: WsClient, msg: WsMessage) -> WsHandlerResult {
        let text = msg
            .as_text()
            .ok_or_else(|| WsError::InvalidMessage("Expected text".into()))?;
        self.broadcast
            .to_all()
            .send(WsMessage::text(text.to_string()))
            .await
            .ok();
        Ok(WsHandlerOutput::Empty)
    }
}

#[module(providers: [RoomGateway], imports: [BroadcastModule::new()])]
struct RoomModule;

// ─────────────────────────────────────────────────────────────────────────────
// Separate-port gateway — `port = 19001` routes via TungsteniteAdapter, not HTTP
// ─────────────────────────────────────────────────────────────────────────────

#[websocket_gateway("/ws", port = 19001)]
pub struct PingGateway {}
#[subscriptions]
impl PingGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [PingGateway])]
struct PingModule;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Full path: TCP upgrade → AxumWsSocket → GatewayWrapper → echo handler → response.
/// Uses the simple `handle_connection()` path (no BroadcastModule).
#[tokio_localset_test::localset_test]
async fn websocket_echo_end_to_end() {
    let server = TestServer::start(EchoModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/echo", server.port);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "message", "data": "hello"}"#.to_string().into(),
    ))
    .await
    .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(
        msg.to_text().unwrap(),
        r#"Echo: {"event": "message", "data": "hello"}"#,
    );
}

/// A gateway declared with no `#[subscriptions]` impl still registers its path and accepts
/// connections — the `WsHandlersBridge` defaults stand in for every behavior method. This is the
/// self-sufficiency guarantee: `#[websocket_gateway]` alone is a valid gateway.
#[tokio_localset_test::localset_test]
async fn websocket_bare_gateway_accepts_connection() {
    let server = TestServer::start(BareModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/bare", server.port);

    let (ws, response) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    assert_eq!(response.status(), 101);
    drop(ws);
}

/// A `#[on_connect]` hook with no `#[subscriptions]` impl is wired in and fires on connect —
/// the connection-hook macros stand on their own, separate from the subscription router.
#[tokio_localset_test::localset_test]
async fn websocket_on_connect_without_subscriptions_fires() {
    let server = TestServer::start(HookOnlyModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/hook-only", server.port);

    let (ws, response) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    assert_eq!(response.status(), 101);

    // on_connect runs server-side once the upgrade completes; poll briefly for the side effect.
    for _ in 0..50 {
        if ON_CONNECT_FIRED.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        ON_CONNECT_FIRED.load(Ordering::SeqCst),
        "#[on_connect] should fire without a #[subscriptions] impl"
    );
    drop(ws);
}

/// Full path: two real TCP clients, `handle_connection_with_broadcast()` path.
/// Race-free: each client handshakes with ping/pong before the broadcast is sent,
/// proving it has passed `complete_connect()` and is registered in `ConnectionManager`.
#[tokio_localset_test::localset_test]
async fn websocket_broadcast_end_to_end() {
    let server = TestServer::start(RoomModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/room", server.port);

    let (mut client_a, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut client_b, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Handshake: wait for both clients to be registered in ConnectionManager.
    // Receiving "pong" means the server's message loop is running, which only
    // starts after begin_connect → CM.register() → complete_connect() have all run.
    for ws in [&mut client_a, &mut client_b] {
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event": "ping"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let pong = ws.next().await.unwrap().unwrap();
        assert_eq!(pong.to_text().unwrap(), "pong");
    }

    // Both clients are now guaranteed to be in ConnectionManager.
    client_a
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event": "shout", "data": "hello room"}"#.to_string().into(),
        ))
        .await
        .unwrap();

    let recv_a = client_a.next().await.unwrap().unwrap();
    let recv_b = client_b.next().await.unwrap().unwrap();

    assert_eq!(
        recv_a.to_text().unwrap(),
        r#"{"event": "shout", "data": "hello room"}"#,
    );
    assert_eq!(
        recv_b.to_text().unwrap(),
        r#"{"event": "shout", "data": "hello room"}"#,
    );
}

#[websocket_gateway("/events")]
pub struct EventGateway {
    #[inject]
    broadcast: BroadcastService,
}
#[subscriptions]
impl EventGateway {
    #[new]
    pub fn new(broadcast: BroadcastService) -> Self {
        Self { broadcast }
    }

    // Called by the REST controller to push a message to all connected WS clients.
    pub async fn push(&self, msg: &str) {
        self.broadcast
            .to_all()
            .send(WsMessage::text(msg.to_string()))
            .await
            .ok();
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[controller("/trigger")]
pub struct TriggerController {
    #[inject]
    gateway: EventGateway,
}

#[routes]
impl TriggerController {
    #[post("/")]
    async fn trigger(&self) -> ToniBody {
        self.gateway.push("server_push").await;
        ToniBody::text("ok".to_string())
    }
}

#[module(
    providers: [EventGateway],
    controllers: [TriggerController],
    imports: [BroadcastModule::new()],
)]
struct GatewayInjectionModule;

/// Gateway injected into a REST controller.
///
/// Verifies that a `#[websocket_gateway]` struct — which is also a DI provider — can be
/// injected as a dependency into an HTTP controller, and that calling a method on the
/// injected instance broadcasts to connected WebSocket clients via the shared
/// `BroadcastService`.
///
/// Flow:
///   1. WS client connects and handshakes (ping/pong) — proves it is registered in ConnectionManager.
///   2. HTTP client POSTs to `/trigger` — controller calls `gateway.push("server_push")`.
///   3. WS client receives `"server_push"` — proves the injected gateway shares the same
///      `ConnectionManager` as the live gateway.
#[tokio_localset_test::localset_test]
async fn gateway_injected_into_rest_controller() {
    let server = TestServer::start(GatewayInjectionModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/events", server.port);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Handshake: receiving "pong" proves the client has passed complete_connect()
    // and is registered in ConnectionManager.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let pong = ws.next().await.unwrap().unwrap();
    assert_eq!(pong.to_text().unwrap(), "pong");

    // Trigger broadcast from the REST handler.
    let resp = server
        .client()
        .post(server.url("/trigger"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The WS client must receive the message pushed by the controller.
    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg.to_text().unwrap(), "server_push");
}

/// Separate-port path: `PingGateway` declares `port = 19001`, so the framework routes it
/// through `TungsteniteAdapter` instead of the HTTP adapter. A client connecting directly
/// to port 19001 exercises the full chain:
///
///   TCP connect → tungstenite handshake → TungsteniteWsSocket → GatewayWrapper → PingGateway
///
/// `PingGateway` hardcodes `port = 19001` in the macro, so this test and any other
/// using `PingModule` must be serialized to avoid binding the same port concurrently.
#[serial]
#[tokio_localset_test::localset_test]
async fn websocket_separate_port_end_to_end() {
    // HTTP server on OS-assigned port; WS gateway listens separately on 19001.
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(PingModule).await;
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        app.use_websocket_adapter(TungsteniteAdapter::new())
            .unwrap();
        let bound = app.bind().await.unwrap();
        let ws_addr = bound
            .websocket
            .into_iter()
            .next()
            .expect("WS adapter not bound");
        let _ = addr_tx.send(ws_addr);
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });

    let ws_addr = addr_rx.await.unwrap();

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", ws_addr))
        .await
        .expect("should connect to separate-port WS server");

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "pong");
}

/// ShutdownHandle must stop the tungstenite server.
/// Verifies via a real TCP connect attempt that the port is no longer listening.
///
/// Shares `PingModule` (port 19001) with `websocket_separate_port_end_to_end`,
/// so it must be serialized.
#[serial]
#[tokio_localset_test::localset_test]
async fn separate_port_close_stops_ws_server() {
    let (addr_tx, addr_rx) =
        tokio::sync::oneshot::channel::<(std::net::SocketAddr, std::net::SocketAddr)>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(PingModule).await;
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        app.use_websocket_adapter(TungsteniteAdapter::new())
            .unwrap();
        let bound = app.bind().await.unwrap();
        let http_addr = bound.http.expect("HTTP adapter not bound");
        let ws_addr = bound
            .websocket
            .into_iter()
            .next()
            .expect("WS adapter not bound");
        let _ = addr_tx.send((http_addr, ws_addr));
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let (http_addr, ws_addr) = addr_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    // Verify the WS server is up and handling messages before shutdown.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", ws_addr))
        .await
        .expect("WS server should be reachable before shutdown");

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "pong");

    // Trigger graceful shutdown and wait for completion.
    shutdown.shutdown();
    shutdown.completed().await;

    // WS port must refuse new connections.
    let result = tokio_tungstenite::connect_async(format!("ws://{}/ws", ws_addr)).await;
    assert!(
        result.is_err(),
        "WS server should be stopped after shutdown"
    );

    // HTTP server must also be stopped.
    let result = reqwest::get(format!("http://{}/", http_addr)).await;
    assert!(
        result.is_err(),
        "HTTP server should be stopped after shutdown"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// `port = 0` on a gateway means "I want my own listener, OS-assigned" — even
// when the HTTP server also requested 0. The same/separate-port partition used
// to compare requested port numbers, which made `Some(0) == Some(0)` route the
// gateway as same-port. Now both bind to distinct OS-assigned listeners.
// ─────────────────────────────────────────────────────────────────────────────

#[websocket_gateway("/zero", port = 0)]
pub struct ZeroPortGateway {}
#[subscriptions]
impl ZeroPortGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [ZeroPortGateway])]
struct ZeroPortModule;

#[tokio_localset_test::localset_test]
async fn gateway_port_zero_binds_separately_from_http_port_zero() {
    let (addr_tx, addr_rx) =
        tokio::sync::oneshot::channel::<(std::net::SocketAddr, std::net::SocketAddr)>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ZeroPortModule).await;
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        app.use_websocket_adapter(TungsteniteAdapter::new())
            .unwrap();
        let bound = app.bind().await.unwrap();
        let http_addr = bound.http.expect("HTTP adapter not bound");
        let ws_addr = bound
            .websocket
            .into_iter()
            .next()
            .expect("WS adapter not bound — gateway with port=0 was misrouted as same-port");
        let _ = addr_tx.send((http_addr, ws_addr));
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });

    let (http_addr, ws_addr) = addr_rx.await.unwrap();

    assert_ne!(
        http_addr.port(),
        ws_addr.port(),
        "HTTP and gateway both requested port 0 but landed on the same listener"
    );

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/zero", ws_addr))
        .await
        .expect("WS connect to OS-assigned separate port");
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "pong");
}
