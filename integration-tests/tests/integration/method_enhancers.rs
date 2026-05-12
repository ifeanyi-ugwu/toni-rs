//! Integration tests for method-level enhancers on WebSocket gateways and RPC controllers.
//!
//! Two comprehensive tests — one per protocol — each verifying all four enhancer
//! types (guard, interceptor, pipe, error handler) at the handler/pattern level.
//!
//! Each test proves two properties per enhancer type:
//! - Correctness: the enhancer produces the expected effect on its annotated handler.
//! - Isolation: a method-level enhancer does not affect sibling handlers ("plain").

use std::time::Duration;

use toni::async_trait;
use toni::context::{HandlerContext, RpcContext, WsContext};
use toni::rpc::{RpcData, RpcError};
use toni::traits_helpers::{ErrorHandler, Guard, Interceptor, InterceptorNext, Pipe};
use toni::websocket::{WsClient, WsError, WsHandlerResult, WsMessage};
use toni::{error_handler, guard, injectable, interceptor, pipe};
use toni::module;
use toni_macros::{rpc_controller, websocket_gateway};

use crate::common::TestServer;

// ---- shared (protocol-agnostic) enhancers ------------------------------------

#[injectable(pub struct AbortPipe {})]
#[pipe(rpc, ws)]
impl AbortPipe {}

impl Pipe<RpcContext> for AbortPipe {
    fn process(&self, ctx: &mut RpcContext) {
        ctx.abort();
    }
}

impl Pipe<WsContext> for AbortPipe {
    fn process(&self, ctx: &mut WsContext) {
        ctx.abort();
    }
}

#[injectable(pub struct RecoveryErrorHandler {})]
#[error_handler(rpc, ws)]
impl RecoveryErrorHandler {}

#[async_trait]
impl ErrorHandler<RpcContext, RpcData> for RecoveryErrorHandler {
    async fn handle_error(
        &self,
        _error: toni::traits_helpers::ChainError<'_>,
        _ctx: &RpcContext,
    ) -> Option<RpcData> {
        Some(RpcData::json(serde_json::json!("recovered")))
    }
}

#[async_trait]
impl ErrorHandler<WsContext, WsMessage> for RecoveryErrorHandler {
    async fn handle_error(
        &self,
        _error: toni::traits_helpers::ChainError<'_>,
        _ctx: &WsContext,
    ) -> Option<WsMessage> {
        Some(WsMessage::text("recovered"))
    }
}

// ---- WS enhancers ------------------------------------------------------------

/// Passes when the WS handshake contains `x-allow: ok`.
#[injectable(pub struct WsAllowGuard {})]
#[guard(ws)]
impl WsAllowGuard {}

#[async_trait]
impl Guard<WsContext> for WsAllowGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        ctx.client()
            .handshake
            .headers
            .get("x-allow")
            .cloned()
            .map_or(false, |v| v == "ok")
    }
}

/// Prefixes the WS text response with "prefixed:".
#[injectable(pub struct WsPrefixInterceptor {})]
#[interceptor(ws)]
impl WsPrefixInterceptor {}

#[async_trait]
impl Interceptor<WsContext> for WsPrefixInterceptor {
    async fn intercept(
        &self,
        ctx: &mut WsContext,
        next: Box<dyn InterceptorNext<WsContext>>,
    ) {
        next.run(ctx).await;
        if let Some(Ok(Some(msg))) = ctx.response() {
            let prefixed = format!("prefixed:{}", msg.as_text().unwrap_or(""));
            ctx.set_response(Ok(Some(WsMessage::text(prefixed))));
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
#[guard(rpc)]
impl RpcAllowGuard {}

#[async_trait]
impl Guard<RpcContext> for RpcAllowGuard {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.data()
            .as_json()
            .and_then(|v| v["allow"].as_str())
            .map(|v| v == "ok")
            .unwrap_or(false)
    }
}

/// Prefixes the RPC string response with "prefixed:".
#[injectable(pub struct RpcPrefixInterceptor {})]
#[interceptor(rpc)]
impl RpcPrefixInterceptor {}

#[async_trait]
impl Interceptor<RpcContext> for RpcPrefixInterceptor {
    async fn intercept(
        &self,
        ctx: &mut RpcContext,
        next: Box<dyn InterceptorNext<RpcContext>>,
    ) {
        next.run(ctx).await;
        let prefixed: Option<String> = ctx
            .response()
            .and_then(|r| r.as_ref().ok())
            .and_then(|opt| opt.as_ref())
            .and_then(|data| data.as_json())
            .and_then(|v| v.as_str())
            .map(|s| format!("prefixed:{}", s));
        if let Some(val) = prefixed {
            ctx.set_response(Ok(Some(RpcData::json(serde_json::json!(val)))));
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
    async fn all(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("all-ok")))
    }

    #[message_pattern("rpc.piped")]
    #[use_pipes(AbortPipe)]
    async fn piped(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("should-not-reach")))
    }

    #[message_pattern("rpc.recovering")]
    #[use_error_handlers(RecoveryErrorHandler)]
    async fn recovering(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Err(RpcError::Internal("intentional".into()))
    }

    #[message_pattern("rpc.plain")]
    async fn plain(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("plain-ok")))
    }
}

#[module(providers: [RpcAllowGuard, RpcPrefixInterceptor, AbortPipe, RecoveryErrorHandler, RpcMethodEnhancersController])]
impl RpcMethodEnhancersModule {}

// ---- TCP helpers -------------------------------------------------------------

/// Pick an OS-assigned free port by binding then dropping a listener.
/// Tiny TOCTOU window — fine for localhost tests, robust under nextest's
/// process-per-test isolation.
async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_rpc_server(module: toni::module_helpers::module_enum::ModuleDefinition) -> u16 {
    use toni::toni_factory::ToniFactory;
    let port = pick_free_port().await;
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.start().await.unwrap();
    });
    tokio::task::spawn_local(async move { local.await });
    port
}

/// `TcpAdapter` binds inside its serve future, so there's no readiness signal
/// from the framework. Retry connect until success or the deadline expires.
async fn connect_with_retry(port: u16) -> tokio::net::TcpStream {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await {
            Ok(s) => return s,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("RPC server never accepted on port {}: {}", port, e),
        }
    }
}

/// Sends one request-response message over a raw TCP connection and returns
/// the parsed frame: `{"id":"1","response":...}` or `{"id":"1","err":{...}}`.
async fn tcp_rpc(port: u16, pattern: &str, data: serde_json::Value) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = connect_with_retry(port).await;
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
        // User-handler `Err(WsError::Internal)` flows through the chain;
        // the method-level `RecoveryErrorHandler` claims it and replaces
        // the response with `"recovered"`. (Without that handler,
        // `WsError::to_message` would render the canonical envelope instead.)
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
/// "rpc.recovering" → handler errors; chain claims via RecoveryErrorHandler
///                    → "recovered"
/// "rpc.plain"      → "plain-ok" always  (isolation control)
#[tokio_localset_test::localset_test]
async fn rpc_method_level_enhancers_work() {
    let port = start_rpc_server(RpcMethodEnhancersModule::module_definition()).await;

    let resp = tcp_rpc(port, "rpc.all", serde_json::json!({"allow": "ok"})).await;
    assert_eq!(resp["response"], "prefixed:all-ok");

    let resp = tcp_rpc(port, "rpc.all", serde_json::json!({})).await;
    assert_eq!(resp["err"]["status"], "forbidden");

    let resp = tcp_rpc(port, "rpc.piped", serde_json::json!({})).await;
    assert!(resp.get("err").is_some(), "pipe should have aborted");

    // User-handler `Err(RpcError::Internal)` flows through the chain;
    // the method-level `RecoveryErrorHandler` claims it and replaces the
    // response with `"recovered"`. (Without that handler, `RpcError::to_data`
    // would render the canonical error envelope instead.)
    let resp = tcp_rpc(port, "rpc.recovering", serde_json::json!({})).await;
    assert_eq!(resp["response"], "recovered");

    let resp = tcp_rpc(port, "rpc.plain", serde_json::json!({})).await;
    assert_eq!(resp["response"], "plain-ok");
}
