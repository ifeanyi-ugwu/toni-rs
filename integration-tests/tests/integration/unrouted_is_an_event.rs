//! Nothing handled the call, and that is a `Unrouted` event.
//!
//! An RPC pattern no controller claims used to be refused at the dispatcher
//! with no context built, so no observer fanned and no error handler was
//! consulted — the one call an operator most wants to hear about was the only
//! one nothing could see. A WebSocket event no handler subscribes to reached the
//! chain already, but as a bare `WsError`, so a catcher had to match a transport
//! type rather than the condition.
//!
//! Unclaimed, both render exactly what they rendered before.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use toni::async_trait;
use toni::context::{RpcContext, WsContext};
use toni::errors::Unrouted;
use toni::rpc::{RpcData, RpcHandlerOutput, RpcHandlerResult};
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::ErrorObserver;
use toni::websocket::{WsHandlerResult, WsMessage};
use toni::{catch, module, Error};
use toni_macros::{
    controller, message_pattern, new, patterns, subscribe_message, subscriptions, websocket_gateway,
};

use crate::common::TestServer;

static OBSERVED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Records every framework error it is handed, by display.
struct RecordingObserver;

#[async_trait]
impl ErrorObserver for RecordingObserver {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn std::error::Error + Send + Sync + 'static),
        _ctx: &'a (dyn toni::context::HandlerContext + 'a),
    ) {
        OBSERVED.lock().unwrap().push(error.to_string());
    }
}

#[catch(Unrouted)]
async fn rpc_unrouted(err: &Unrouted, _ctx: &RpcContext) -> RpcData {
    RpcData::from_serialize(&serde_json::json!({ "missing": err.target })).unwrap()
}

#[catch(Unrouted)]
async fn ws_unrouted(err: &Unrouted, _ctx: &WsContext) -> WsMessage {
    WsMessage::text(format!("missing:{}", err.target))
}

// ── RPC ────────────────────────────────────────────────────────────────────

#[controller]
pub struct SomethingController {}

#[patterns]
impl SomethingController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("routed.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Single(RpcData::text("routed")))
    }
}

#[module(controllers: [SomethingController])]
impl UnroutedRpcModule {}

async fn boot_rpc<F>(configure: F) -> u16
where
    F: FnOnce(&mut ToniFactory) + Send + 'static,
{
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(UnroutedRpcModule).await.unwrap();
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

async fn call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

/// The miss reaches an observer, which is what it never did.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unrouted_rpc_pattern_reaches_the_observers() {
    OBSERVED.lock().unwrap().clear();

    let port = boot_rpc(|f| {
        f.use_global_error_observer(Arc::new(RecordingObserver));
    })
    .await;
    call(port, "nobody.claims.this").await;

    assert_eq!(
        OBSERVED.lock().unwrap().clone(),
        vec!["nothing handles nobody.claims.this"]
    );
}

/// And a handler can claim it, which means the chain was reached.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unrouted_rpc_pattern_is_claimable() {
    let port = boot_rpc(|f| {
        f.use_global_rpc_error_handler(Arc::new(rpc_unrouted));
    })
    .await;
    let reply = call(port, "nobody.claims.this").await;

    assert_eq!(
        reply["response"]["missing"], "nobody.claims.this",
        "reply: {reply}"
    );
}

/// Unclaimed, the caller sees the frame it always saw.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unclaimed_rpc_miss_renders_as_before() {
    let port = boot_rpc(|_| {}).await;
    let reply = call(port, "nobody.claims.this").await;

    assert_eq!(reply["err"]["status"], "not_found", "reply: {reply}");
}

// ── WebSocket ──────────────────────────────────────────────────────────────

#[websocket_gateway("/ws-unrouted")]
pub struct SomethingGateway {}

#[subscriptions]
impl SomethingGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("routed")]
    async fn routed(&self) -> WsHandlerResult {
        Ok(WsMessage::text("routed").into())
    }
}

#[module(providers: [SomethingGateway])]
impl UnroutedWsModule {}

async fn ask_ws(factory: ToniFactory, event: &str) -> String {
    let server = TestServer::start_with(factory, UnroutedWsModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-unrouted", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        format!(r#"{{"event":"{event}"}}"#).into(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("a reply must arrive")
        .expect("the socket stays open")
        .expect("the frame arrives")
        .into_text()
        .unwrap()
        .to_string()
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_unrouted_ws_event_is_claimable() {
    let mut factory = ToniFactory::new();
    factory.use_global_ws_error_handler(Arc::new(ws_unrouted));

    assert_eq!(
        ask_ws(factory, "nobody-claims-this").await,
        "missing:nobody-claims-this"
    );
}

/// Unclaimed, the envelope is the one it always was.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unclaimed_ws_miss_renders_as_before() {
    let reply = ask_ws(ToniFactory::new(), "nobody-claims-this").await;
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("an error envelope");

    assert_eq!(reply["status"], "error");
    assert_eq!(reply["kind"], "NotFound", "envelope: {reply}");
}
