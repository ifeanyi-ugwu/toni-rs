//! What an enhancer attaches to the message's extension bag reaches the handler.
//!
//! Both halves of the HTTP path are covered — a middleware writing before
//! routing, and a guard writing after it — plus the WebSocket path, where the
//! handler receives a `WsClient` rather than the context. `rpc_tcp.rs` covers
//! the RPC path, whose handler holds the context already.
//!
//! `guard_mut_context.rs` covers the enhancer-to-enhancer half.

use toni::async_trait;
use toni::context::{Extensions, HandlerContext, HttpContext, WsContext};
use toni::extractors::{Bytes as ToniBytes, Path};
use toni::middleware::{Middleware, MiddlewareResult, NextHandle};
use toni::traits_helpers::Guard;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    controller, get, injectable, module, new, post, routes, set_metadata, subscriptions,
    toni_factory::ToniFactory, websocket_gateway, Body as ToniBody,
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
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
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
    async fn can_activate(&self, ctx: &WsContext) -> bool {
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

    /// The handler asks for the bag by name. It reaches the guard's write without the client
    /// carrying it, which is the same shape every other transport uses.
    #[subscribe_message("read")]
    #[use_guards(WsAuthGuard)]
    async fn read(&self, ext: Extensions, _m: WsMessage) -> WsHandlerResult {
        let principal = ext
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

// ===== the context as a handler parameter =====

#[injectable]
pub struct StampGuard {}

#[async_trait]
impl Guard<HttpContext> for StampGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.extensions().insert(Principal("dana".into()));
        true
    }
}

#[derive(Clone)]
pub struct Role(&'static str);

#[controller("/ctx")]
pub struct CtxController {}

#[routes]
#[use_guards(StampGuard)]
impl CtxController {
    /// Takes the context itself rather than an extractor over it.
    #[get("/read")]
    #[set_metadata(Role("reader"))]
    fn read(&self, ctx: &HttpContext) -> ToniBody {
        let principal = ctx
            .extensions()
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        // Route metadata reaches the handler for the first time here — it lives
        // on the context and nothing else carried it.
        let role = ctx
            .route_metadata()
            .and_then(|m| m.get::<Role>())
            .map(|r| r.0)
            .unwrap_or("none");
        ToniBody::text(format!("{principal}/{role}"))
    }

    /// The context coexists with ordinary extractors.
    #[get("/with-path/{id}")]
    fn with_path(&self, Path(id): Path<u32>, ctx: &HttpContext) -> ToniBody {
        let principal = ctx
            .extensions()
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        ToniBody::text(format!("{id}/{principal}"))
    }
}

#[module(controllers: [CtxController], providers: [StampGuard])]
impl CtxModule {}

#[tokio_localset_test::localset_test]
async fn a_handler_can_take_the_context() {
    let server = TestServer::start(CtxModule).await;

    let body = server
        .client()
        .get(server.url("/ctx/read"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "dana/reader");
}

#[tokio_localset_test::localset_test]
async fn the_context_coexists_with_extractors() {
    let server = TestServer::start(CtxModule).await;

    let body = server
        .client()
        .get(server.url("/ctx/with-path/7"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "7/dana");
}

// ===== the request body is single-use, and says so =====

/// Reads the raw body to check a signature — the reason `take_request` is public
/// on the context in the first place.
#[injectable]
pub struct BodyReadingGuard {}

#[async_trait]
impl Guard<HttpContext> for BodyReadingGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        let first = ctx.take_request().is_some();
        // Gone on the second look, and the type says so rather than handing
        // back an empty body.
        let second = ctx.take_request().is_some();
        ctx.extensions().insert(BodySeen { first, second });
        true
    }
}

#[derive(Clone)]
pub struct BodySeen {
    first: bool,
    second: bool,
}

#[controller("/body")]
pub struct BodyController {}

#[routes]
#[use_guards(BodyReadingGuard)]
impl BodyController {
    /// Reads no body itself, so it runs and can report what the guard saw.
    #[post("/seen")]
    fn seen(&self, ext: Extensions) -> ToniBody {
        let seen = ext.get::<BodySeen>().expect("guard runs first");
        ToniBody::text(format!("{}/{}", seen.first, seen.second))
    }

    /// Wants the body the guard already took.
    #[post("/wants-body")]
    fn wants_body(&self, body: ToniBytes) -> ToniBody {
        ToniBody::text(format!("{}", body.0.len()))
    }
}

#[module(controllers: [BodyController], providers: [BodyReadingGuard])]
impl BodyModule {}

#[tokio_localset_test::localset_test]
async fn an_enhancer_reads_the_body_once_and_sees_it_gone() {
    let server = TestServer::start(BodyModule).await;

    let body = server
        .client()
        .post(server.url("/body/seen"))
        .body("payload")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "true/false");
}

#[tokio_localset_test::localset_test]
async fn a_handler_whose_body_was_taken_is_told_which_extractor_lost() {
    let server = TestServer::start(BodyModule).await;

    let resp = server
        .client()
        .post(server.url("/body/wants-body"))
        .body("payload")
        .send()
        .await
        .unwrap();

    // Not an empty body handed over in silence — the extractor that came up
    // short is named.
    assert_eq!(resp.status(), 400);
    let text = resp.text().await.unwrap();
    assert!(text.contains("Bytes"), "{text}");
    assert!(text.contains("already read"), "{text}");
}

// A handler answering both ways used to need a precedence rule, and a warning
// when it was applied. Returning is now the only way to answer, so there is no
// precedence left to pin.

// ===== the execution outlives the handler =====

#[derive(Clone)]
pub struct Alive(std::sync::Arc<std::sync::atomic::AtomicBool>);

/// Sets the flag when the bag holding it is dropped, which is when the
/// execution's state goes away.
pub struct Sentinel(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Sentinel {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[injectable]
pub struct SentinelGuard {}

#[async_trait]
impl Guard<HttpContext> for SentinelGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        ctx.extensions().insert(Sentinel(flag.clone()));
        ctx.extensions().insert(Alive(flag));
        ctx.extensions().insert(Principal("erin".into()));
        true
    }
}

#[controller("/tail")]
pub struct TailController {}

#[routes]
#[use_guards(SentinelGuard)]
impl TailController {
    /// Holds the context, so the `Arc` alone keeps the bag reachable.
    #[get("/captured")]
    fn captured(&self, ctx: &HttpContext) -> ToniBody {
        use futures_util::StreamExt;
        let held = ctx.clone();
        ToniBody::stream(futures_util::stream::iter(0..3).map(move |i| {
            let who = held
                .extensions()
                .get::<Principal>()
                .map(|p| p.0)
                .unwrap_or_else(|| "ABSENT".into());
            Ok::<_, std::io::Error>(bytes::Bytes::from(format!("{i}:{who};")))
        }))
    }

    /// Holds only the flag — never the context. Whether the execution's state
    /// survives the drain is then a property of the framework rather than of
    /// what this handler happened to capture.
    #[get("/detached")]
    fn detached(&self, ctx: &HttpContext) -> ToniBody {
        use futures_util::StreamExt;
        let flag = ctx.extensions().get::<Alive>().expect("guard ran").0;
        ToniBody::stream(futures_util::stream::iter(0..3).map(move |i| {
            let state = if flag.load(std::sync::atomic::Ordering::SeqCst) {
                "alive"
            } else {
                "dropped"
            };
            Ok::<_, std::io::Error>(bytes::Bytes::from(format!("{i}:{state};")))
        }))
    }
}

#[module(controllers: [TailController], providers: [SentinelGuard])]
impl TailModule {}

/// A stream that captures the context reads the bag through its own `Arc`. This
/// holds because a context is a handle, with no help from the dispatcher.
#[tokio_localset_test::localset_test]
async fn a_capturing_stream_reads_the_bag() {
    let server = TestServer::start(TailModule).await;

    let body = server
        .client()
        .get(server.url("/tail/captured"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "0:erin;1:erin;2:erin;");
}

/// The execution's state is still there while the body streams, even though
/// nothing in the stream holds the context. The response body carries it, so
/// the execution ends with the answer rather than with the handler.
#[tokio_localset_test::localset_test]
async fn execution_state_survives_until_the_body_is_drained() {
    let server = TestServer::start(TailModule).await;

    let body = server
        .client()
        .get(server.url("/tail/detached"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "0:alive;1:alive;2:alive;");
}
