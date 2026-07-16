//! Integration tests for request- and transient-scoped providers acting as
//! guards and interceptors. These verify that scope does not prevent a provider
//! from contributing to the enhancer pipeline — a fresh instance is constructed
//! per request using the DynGuardFactory / DynInterceptorFactory path.

use toni::async_trait;
use toni::context::{HttpContext, WsContext};
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    controller, get, injectable, module, routes, use_guards, use_interceptors, Body as ToniBody,
    Request,
};
use toni_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;

// ---- request-scoped guard, no injected deps ----------------------------------

#[injectable(scope = "request")]
pub struct RequestGuard {}
impl RequestGuard {}

#[async_trait]
impl Guard<HttpContext> for RequestGuard {
    async fn can_activate(&self, context: &mut HttpContext) -> bool {
        context.request().headers.contains_key("x-allow")
    }
}

// ---- request-scoped guard that injects Request -------------------------------

#[injectable(scope = "request")]
pub struct HeaderGuard {
    #[inject]
    request: Request,
}
impl HeaderGuard {}

#[async_trait]
impl Guard<HttpContext> for HeaderGuard {
    async fn can_activate(&self, _context: &mut HttpContext) -> bool {
        self.request
            .header("x-secret")
            .map_or(false, |v| v == "open-sesame")
    }
}

// ---- transient-scoped interceptor --------------------------------------------

#[injectable(scope = "transient")]
pub struct TransientInterceptor {}
impl TransientInterceptor {}

#[async_trait]
impl Interceptor<HttpContext> for TransientInterceptor {
    async fn intercept(
        &self,
        context: &mut HttpContext,
        next: Box<dyn InterceptorNext<HttpContext>>,
    ) {
        next.run(context).await;
        context
            .response_mut()
            .unwrap()
            .headers
            .push(("x-transient".to_string(), "hit".to_string()));
    }
}

// ---- controllers -------------------------------------------------------------

#[controller("/gate")]
pub struct GateController {}

#[routes]
#[use_guards(RequestGuard)]
impl GateController {
    #[get("/check")]
    fn check(&self) -> ToniBody {
        ToniBody::text("passed".to_string())
    }
}

#[controller("/secret")]
pub struct SecretController {}

#[routes]
#[use_guards(HeaderGuard)]
impl SecretController {
    #[get("/unlock")]
    fn unlock(&self) -> ToniBody {
        ToniBody::text("unlocked".to_string())
    }
}

#[controller("/transient")]
pub struct TransientController {}

#[routes]
#[use_interceptors(TransientInterceptor)]
impl TransientController {
    #[get("/ping")]
    fn ping(&self) -> ToniBody {
        ToniBody::text("pong".to_string())
    }
}

// ---- modules -----------------------------------------------------------------

#[module(
    controllers: [GateController],
    providers: [RequestGuard],
)]
impl RequestGuardModule {}

#[module(
    controllers: [SecretController],
    providers: [HeaderGuard],
)]
impl HeaderGuardModule {}

#[module(
    controllers: [TransientController],
    providers: [TransientInterceptor],
)]
impl TransientInterceptorModule {}

// ---- WS gateway with handshake guard -----------------------------------------
//
// WsHandshakeGuard reads `x-auth-token` from the WsClient handshake headers,
// which are populated from the HTTP upgrade request by create_client_from_parts.
// This is the correct multi-context guard pattern: it reads from WsClient so it
// works identically at connect time and per-message, with no HTTP Request injection.

#[injectable]
pub struct WsHandshakeGuard {}
impl WsHandshakeGuard {}

#[async_trait]
impl Guard<WsContext> for WsHandshakeGuard {
    async fn can_activate(&self, ctx: &mut WsContext) -> bool {
        ctx.client()
            .handshake
            .headers
            .get("x-auth-token")
            .cloned()
            .map_or(false, |v| v == "secret")
    }
}

#[websocket_gateway("/guarded-ws")]
pub struct GuardedGateway {}
#[subscriptions]
#[use_guards(WsHandshakeGuard)]
impl GuardedGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [WsHandshakeGuard, GuardedGateway])]
impl WsGuardModule {}

// ---- tests -------------------------------------------------------------------

#[tokio_localset_test::localset_test]
async fn request_scoped_guard_activates() {
    let server = TestServer::start(RequestGuardModule).await;

    // Missing header — guard blocks
    let resp = server
        .client()
        .get(server.url("/gate/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Header present — guard passes
    let resp = server
        .client()
        .get(server.url("/gate/check"))
        .header("x-allow", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "passed");
}

#[tokio_localset_test::localset_test]
async fn request_scoped_guard_injects_request() {
    let server = TestServer::start(HeaderGuardModule).await;

    // Wrong secret — rejected
    let resp = server
        .client()
        .get(server.url("/secret/unlock"))
        .header("x-secret", "wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Correct secret — allowed
    let resp = server
        .client()
        .get(server.url("/secret/unlock"))
        .header("x-secret", "open-sesame")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "unlocked");
}

#[tokio_localset_test::localset_test]
async fn transient_scoped_interceptor() {
    let server = TestServer::start(TransientInterceptorModule).await;

    let resp = server
        .client()
        .get(server.url("/transient/ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-transient").unwrap(), "hit");
    assert_eq!(resp.text().await.unwrap(), "pong");
}

/// A request-scoped guard on a WS gateway reads the HTTP upgrade handshake headers.
///
/// The guard injects `Request` (built from the upgrade `RequestPart`) and checks
/// `x-auth-token`. This exercises the full path:
/// Axum upgrade parts → WsConnectionCallbacks → begin_connect → DynGuardFactory::create(Some(parts))
#[tokio_localset_test::localset_test]
async fn ws_request_scoped_guard_uses_handshake_header() {
    use futures_util::{SinkExt, StreamExt};

    let server = TestServer::start(WsGuardModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/guarded-ws", server.port);

    // Without the token — guard rejects, server closes connection immediately.
    {
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let next = ws.next().await;
        // Server closes the socket; stream yields None or a close frame.
        let closed = match next {
            None => true,
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => true,
            Some(Err(_)) => true,
            _ => false,
        };
        assert!(
            closed,
            "expected server to close connection when guard rejects"
        );
    }

    // With the correct token — guard passes, ping/pong works.
    {
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;
        let req = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&ws_url)
            .header("Host", format!("127.0.0.1:{}", server.port))
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("x-auth-token", "secret")
            .body(())
            .unwrap();
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"event": "ping"}"#.to_string().into(),
        ))
        .await
        .unwrap();

        let reply = ws.next().await.unwrap().unwrap();
        assert_eq!(reply.to_text().unwrap(), "pong");
    }
}
