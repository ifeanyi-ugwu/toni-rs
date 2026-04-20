//! Integration tests for method-level enhancers on WebSocket gateways and RPC controllers.
//!
//! Two comprehensive tests — one per protocol — each verifying all four enhancer
//! types (guard, interceptor, pipe, error handler) at the handler/pattern level.
//!
//! Each test proves two properties per enhancer type:
//! - Correctness: the enhancer produces the expected effect on its annotated handler.
//! - Isolation: a method-level enhancer does not affect sibling handlers ("plain").

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use serial_test::serial;
use toni::async_trait;
use toni::injector::Context;
use toni::rpc::{RpcContext as RpcCallContext, RpcData, RpcError};
use toni::traits_helpers::{
    ErrorHandler, ErrorResponse, Guard, Interceptor, InterceptorNext, Pipe,
};
use toni::websocket::{WsClient, WsError, WsHandlerResult, WsMessage};
use toni::{error_handler, guard, injectable, interceptor, pipe};
use toni::module;
use toni_macros::{rpc_controller, websocket_gateway};

use crate::common::TestServer;

// ---- shared (protocol-agnostic) enhancers ------------------------------------

#[injectable(pub struct AbortPipe {})]
#[pipe]
impl AbortPipe {}

impl Pipe for AbortPipe {
    fn process(&self, context: &mut Context) {
        context.abort();
    }
}

#[injectable(pub struct RecoveryErrorHandler {})]
#[error_handler]
impl RecoveryErrorHandler {}

#[async_trait]
impl ErrorHandler for RecoveryErrorHandler {
    async fn handle_error(
        &self,
        _error: Box<dyn std::error::Error + Send>,
        ctx: &Context,
    ) -> Option<ErrorResponse> {
        if ctx.switch_to_ws().is_some() {
            Some(ErrorResponse::Ws(WsMessage::text("recovered")))
        } else {
            ctx.switch_to_rpc()
                .map(|_| ErrorResponse::Rpc(RpcData::json(serde_json::json!("recovered"))))
        }
    }
}

// ---- WS enhancers ------------------------------------------------------------

/// Passes when the WS handshake contains `x-allow: ok`.
#[injectable(pub struct WsAllowGuard {})]
#[guard]
impl WsAllowGuard {}

#[async_trait]
impl Guard for WsAllowGuard {
    async fn can_activate(&self, context: &Context) -> bool {
        context
            .switch_to_ws()
            .and_then(|ws| ws.client().handshake.headers.get("x-allow").cloned())
            .map_or(false, |v| v == "ok")
    }
}

/// Prefixes the WS text response with "prefixed:".
#[injectable(pub struct WsPrefixInterceptor {})]
#[interceptor]
impl WsPrefixInterceptor {}

#[async_trait]
impl Interceptor for WsPrefixInterceptor {
    async fn intercept(&self, context: &mut Context, next: Box<dyn InterceptorNext>) {
        next.run(context).await;
        let current = context.switch_to_ws().and_then(|ws| ws.response());
        if let Some(Ok(Some(msg))) = current {
            let prefixed = format!("prefixed:{}", msg.as_text().unwrap_or(""));
            context
                .switch_to_ws_mut()
                .expect("WS context required")
                .set_response(Ok(Some(WsMessage::text(prefixed))));
        }
    }
}

// ---- WS gateway --------------------------------------------------------------
//
// Four handlers:
//   "all"        – guard + interceptor (guard + interceptor correctness + isolation)
//   "piped"      – pipe (pipe correctness + isolation)
//   "recovering" – error handler (error handler correctness + isolation)
//   "plain"      – no enhancers (shared isolation control for all three above)

#[websocket_gateway("/ws-method-enhancers", pub struct WsMethodEnhancersGateway {})]
impl WsMethodEnhancersGateway {
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("all")]
    #[use_guards(WsAllowGuard)]
    #[use_interceptors(WsPrefixInterceptor)]
    async fn on_all(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("all-ok").into())
    }

    #[subscribe_message("piped")]
    #[use_pipes(AbortPipe)]
    async fn on_piped(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("should-not-reach").into())
    }

    #[subscribe_message("recovering")]
    #[use_error_handlers(RecoveryErrorHandler)]
    async fn on_recovering(
        &self,
        _c: WsClient,
        _m: WsMessage,
    ) -> WsHandlerResult {
        Err(WsError::Internal("intentional".into()))
    }

    #[subscribe_message("plain")]
    async fn on_plain(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("plain-ok").into())
    }
}

#[module(providers: [WsAllowGuard, WsPrefixInterceptor, AbortPipe, RecoveryErrorHandler, WsMethodEnhancersGateway])]
impl WsMethodEnhancersModule {}

// ---- RPC enhancers -----------------------------------------------------------

/// Passes when the RPC payload contains `{"allow": "ok"}`.
#[injectable(pub struct RpcAllowGuard {})]
#[guard]
impl RpcAllowGuard {}

#[async_trait]
impl Guard for RpcAllowGuard {
    async fn can_activate(&self, context: &Context) -> bool {
        context
            .switch_to_rpc()
            .and_then(|rpc| {
                rpc.data()
                    .as_json()
                    .and_then(|v| v["allow"].as_str())
                    .map(|v| v == "ok")
            })
            .unwrap_or(false)
    }
}

/// Prefixes the RPC string response with "prefixed:".
#[injectable(pub struct RpcPrefixInterceptor {})]
#[interceptor]
impl RpcPrefixInterceptor {}

#[async_trait]
impl Interceptor for RpcPrefixInterceptor {
    async fn intercept(&self, context: &mut Context, next: Box<dyn InterceptorNext>) {
        next.run(context).await;
        let prefixed: Option<String> = context
            .switch_to_rpc()
            .and_then(|rpc| rpc.response())
            .and_then(|r| r.as_ref().ok())
            .and_then(|opt| opt.as_ref())
            .and_then(|data| data.as_json())
            .and_then(|v| v.as_str())
            .map(|s| format!("prefixed:{}", s));
        if let Some(val) = prefixed {
            context
                .switch_to_rpc_mut()
                .expect("RPC context required")
                .set_response(Ok(Some(RpcData::json(serde_json::json!(val)))));
        }
    }
}

// ---- RPC controller ----------------------------------------------------------
//
// Same four-handler shape as the WS gateway.

#[rpc_controller(pub struct RpcMethodEnhancersController {})]
impl RpcMethodEnhancersController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.all")]
    #[use_guards(RpcAllowGuard)]
    #[use_interceptors(RpcPrefixInterceptor)]
    async fn all(&self, _d: RpcData, _c: RpcCallContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("all-ok")))
    }

    #[message_pattern("rpc.piped")]
    #[use_pipes(AbortPipe)]
    async fn piped(&self, _d: RpcData, _c: RpcCallContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("should-not-reach")))
    }

    #[message_pattern("rpc.recovering")]
    #[use_error_handlers(RecoveryErrorHandler)]
    async fn recovering(&self, _d: RpcData, _c: RpcCallContext) -> Result<RpcData, RpcError> {
        Err(RpcError::Internal("intentional".into()))
    }

    #[message_pattern("rpc.plain")]
    async fn plain(&self, _d: RpcData, _c: RpcCallContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("plain-ok")))
    }
}

#[module(providers: [RpcAllowGuard, RpcPrefixInterceptor, AbortPipe, RecoveryErrorHandler, RpcMethodEnhancersController])]
impl RpcMethodEnhancersModule {}

// ---- TCP helpers -------------------------------------------------------------

static RPC_PORT: AtomicU16 = AtomicU16::new(31000);

async fn start_rpc_server(module: toni::module_helpers::module_enum::ModuleDefinition) -> u16 {
    use toni::toni_factory::ToniFactory;
    let port = RPC_PORT.fetch_add(1, Ordering::SeqCst);
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        let _ = app.start().await;
    });
    tokio::task::spawn_local(async move { local.await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    port
}

/// Sends one request-response message over a raw TCP connection and returns
/// the parsed frame: `{"id":"1","response":...}` or `{"id":"1","err":{...}}`.
async fn tcp_rpc(port: u16, pattern: &str, data: serde_json::Value) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

// ---- tests -------------------------------------------------------------------

/// All four method-level enhancer types work correctly on a WS gateway.
///
/// With `x-allow: ok` in the handshake:
///   "all"        → guard passes + interceptor prefixes → "prefixed:all-ok"
///   "piped"      → pipe aborts → error frame
///   "recovering" → handler errors, error handler recovers → "recovered"
///   "plain"      → no enhancers → "plain-ok"  (isolation: nothing leaked)
///
/// Without `x-allow`:
///   "all"   → guard blocks silently (no reply)
///   "plain" → still "plain-ok"  (guard is isolated to "all")
#[serial]
#[tokio_localset_test::localset_test]
async fn ws_method_level_enhancers_work() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::handshake::client::generate_key;

    let server = TestServer::start(WsMethodEnhancersModule::module_definition()).await;
    let ws_url = format!("ws://127.0.0.1:{}/ws-method-enhancers", server.port);

    // --- With x-allow: ok ---
    {
        let req = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&ws_url)
            .header("Host", format!("127.0.0.1:{}", server.port))
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("x-allow", "ok")
            .body(())
            .unwrap();
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"all"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(reply.to_text().unwrap(), "prefixed:all-ok");

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"piped"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
        assert!(json.get("error").is_some(), "pipe should have aborted");

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"recovering"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(reply.to_text().unwrap(), "recovered");

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"plain"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(reply.to_text().unwrap(), "plain-ok");
    }

    // --- Without x-allow: guard blocks "all", plain unaffected ---
    {
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"all"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let no_reply = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            no_reply.is_err(),
            "guard should have silently blocked \"all\""
        );

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event":"plain"}"#.to_string().into(),
        ))
        .await
        .unwrap();
        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(
            reply.to_text().unwrap(),
            "plain-ok",
            "guard must not affect sibling handlers"
        );
    }
}

/// All four method-level enhancer types work correctly on an RPC controller via TCP.
///
/// "rpc.all" (guard + interceptor):
///   - `{"allow":"ok"}` → guard passes, interceptor prefixes → "prefixed:all-ok"
///   - `{}`             → guard blocks → Forbidden
///
/// "rpc.piped"      → pipe aborts → err frame
/// "rpc.recovering" → error handler recovers → "recovered"
/// "rpc.plain"      → "plain-ok" always  (isolation control)
#[serial]
#[tokio_localset_test::localset_test]
async fn rpc_method_level_enhancers_work() {
    let port = start_rpc_server(RpcMethodEnhancersModule::module_definition()).await;

    let resp = tcp_rpc(port, "rpc.all", serde_json::json!({"allow": "ok"})).await;
    assert_eq!(resp["response"], "prefixed:all-ok");

    let resp = tcp_rpc(port, "rpc.all", serde_json::json!({})).await;
    assert_eq!(resp["err"]["status"], "forbidden");

    let resp = tcp_rpc(port, "rpc.piped", serde_json::json!({})).await;
    assert!(resp.get("err").is_some(), "pipe should have aborted");

    let resp = tcp_rpc(port, "rpc.recovering", serde_json::json!({})).await;
    assert_eq!(resp["response"], "recovered");

    let resp = tcp_rpc(port, "rpc.plain", serde_json::json!({})).await;
    assert_eq!(resp["response"], "plain-ok");
}
