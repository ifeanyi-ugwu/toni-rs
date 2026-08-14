//! What an enhancer attaches to the message's extension bag reaches the handler.
//!
//! Both halves of the HTTP path are covered — a middleware writing before
//! routing, and a guard writing after it — plus the WebSocket path, where the
//! handler receives a `WsClient` rather than the context. `rpc_tcp.rs` covers
//! the RPC path, whose handler takes `&RpcContext` directly.
//!
//! `guard_mut_context.rs` covers the enhancer-to-enhancer half.

use toni::async_trait;
use toni::context::{Extensions, HandlerContext, HttpContext, WsContext};
use toni::middleware::{Middleware, MiddlewareResult, NextHandle};
use toni::traits_helpers::Guard;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    controller, get, injectable, module, new, routes, subscribe_message, subscriptions,
    toni_factory::ToniFactory, use_guards, websocket_gateway, Body as ToniBody,
};

use crate::common::TestServer;

#[derive(Clone, Debug, PartialEq)]
pub struct Principal(String);

#[derive(Clone, Debug, PartialEq)]
pub struct TraceId(String);

// ===== HTTP =====

#[injectable]
pub struct AuthGuard {}

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, ctx: &mut HttpContext) -> bool {
        ctx.extensions().insert(Principal("alice".into()));
        true
    }
}

/// Runs before route resolution, so its write has to survive routing to be
/// readable by the handler.
struct TracingMiddleware;

#[async_trait]
impl Middleware for TracingMiddleware {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        Extensions::adopt(next.request().extensions()).insert(TraceId("t-42".into()));
        next.run().await
    }
}

#[controller("/bus")]
pub struct BusController {}

#[routes]
#[use_guards(AuthGuard)]
impl BusController {
    #[get("/read")]
    fn read(&self, ext: Extensions) -> ToniBody {
        let principal = ext
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        let trace = ext
            .get::<TraceId>()
            .map(|t| t.0)
            .unwrap_or_else(|| "ABSENT".into());
        ToniBody::text(format!("{principal}/{trace}"))
    }
}

#[module(controllers: [BusController], providers: [AuthGuard])]
impl HttpBusModule {}

#[tokio_localset_test::localset_test]
async fn http_guard_and_middleware_writes_reach_the_handler() {
    let mut factory = ToniFactory::new();
    factory.use_global_middleware(std::sync::Arc::new(TracingMiddleware));
    let server = TestServer::start_with(factory, HttpBusModule).await;

    let body = server
        .client()
        .get(server.url("/bus/read"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "alice/t-42");
}

#[tokio_localset_test::localset_test]
async fn http_bag_does_not_leak_between_requests() {
    let server = TestServer::start(HttpBusModule).await;

    // No middleware registered, so `TraceId` is absent on every request. If the
    // bag were shared beyond one request the guard's `Principal` would also
    // accumulate rather than being rewritten.
    for _ in 0..2 {
        let body = server
            .client()
            .get(server.url("/bus/read"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "alice/ABSENT");
    }
}

// ===== WebSocket =====

#[injectable]
pub struct WsAuthGuard {}

#[async_trait]
impl Guard<WsContext> for WsAuthGuard {
    async fn can_activate(&self, ctx: &mut WsContext) -> bool {
        ctx.extensions().insert(Principal("bob".into()));
        true
    }
}

#[websocket_gateway("/ws-bus")]
pub struct BusGateway {}

#[subscriptions]
impl BusGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The handler gets a `WsClient`, never the context — the bag rides the
    /// client to bridge that gap.
    #[subscribe_message("read")]
    #[use_guards(WsAuthGuard)]
    async fn read(&self, client: WsClient, _m: WsMessage) -> WsHandlerResult {
        let principal = client
            .extensions
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        Ok(WsMessage::text(principal).into())
    }
}

#[module(providers: [WsAuthGuard, BusGateway])]
impl WsBusModule {}

#[tokio_localset_test::localset_test]
async fn ws_guard_write_reaches_the_handler() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(WsBusModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-bus", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    ws.send(Message::Text(r#"{"event":"read"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.into_text().unwrap().as_str(), "bob");
}
