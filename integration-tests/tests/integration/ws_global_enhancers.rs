//! Global WebSocket enhancers, registered on the factory rather than named on a
//! gateway.
//!
//! `use_global_ws_guards`, `use_global_ws_interceptors` and
//! `use_global_ws_error_handler` have existed without coverage, so nothing would
//! have said if they stopped reaching the pipeline.
//!
//! A WebSocket execution is one message (ADR-0016), and the connection itself is
//! one too — so a guard sees `connect` before it sees any event, which the
//! recorded order below shows rather than hides.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::context::WsContext;
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use toni::websocket::{WsError, WsHandlerResult, WsMessage};
use toni::{injectable, module};
use toni_macros::{new, subscribe_message, subscriptions, use_guards, websocket_gateway};

use crate::common::TestServer;

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: String) {
    SEEN.lock().unwrap().push(what);
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

struct GlobalGuard;

#[async_trait]
impl Guard<WsContext> for GlobalGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        record(format!("global:guard:{}", ctx.event()));
        true
    }
}

/// Refuses messages and admits the connection, so the rejection is observable
/// on a socket that is still open.
struct DenyingGlobalGuard;

#[async_trait]
impl Guard<WsContext> for DenyingGlobalGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        record(format!("global:deny:{}", ctx.event()));
        ctx.event() == "connect"
    }
}

struct GlobalInterceptor;

#[async_trait]
impl Interceptor<WsContext, WsHandlerResult> for GlobalInterceptor {
    async fn intercept(
        &self,
        ctx: &WsContext,
        next: Box<dyn InterceptorNext<WsContext, WsHandlerResult>>,
    ) -> WsHandlerResult {
        record("global:before".to_string());
        let answer = next.run(ctx).await;
        record("global:after".to_string());
        answer
    }
}

struct GlobalErrorHandler;

#[async_trait]
impl ErrorHandler<WsContext, WsMessage> for GlobalErrorHandler {
    async fn handle_error(&self, _error: ChainError<'_>, _ctx: &WsContext) -> Option<WsMessage> {
        record("global:error_handler".to_string());
        Some(WsMessage::text("claimed globally"))
    }
}

#[injectable]
pub struct GatewayGuard {}

#[async_trait]
impl Guard<WsContext> for GatewayGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        record(format!("gateway:guard:{}", ctx.event()));
        true
    }
}

#[websocket_gateway("/ws-globals")]
pub struct GlobalsGateway {}

#[subscriptions]
#[use_guards(GatewayGuard)]
impl GlobalsGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self) -> WsHandlerResult {
        record("handler".to_string());
        Ok(WsMessage::text("pong").into())
    }

    #[subscribe_message("boom")]
    async fn boom(&self) -> WsHandlerResult {
        record("handler".to_string());
        Err(WsError::Internal("handler said no".into()))
    }
}

#[module(providers: [GlobalsGateway, GatewayGuard])]
impl GlobalsWsModule {}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Answers `<no reply>` rather than waiting forever: a guard that refuses may
/// legitimately send nothing back, and a test that hangs says less than one
/// that reports what arrived.
async fn ask(ws: &mut Socket, event: &str) -> String {
    ws.send(Message::Text(format!(r#"{{"event":"{event}"}}"#).into()))
        .await
        .unwrap();
    match tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await {
        Ok(Some(Ok(message))) => message.into_text().unwrap().to_string(),
        Ok(other) => format!("<closed: {other:?}>"),
        Err(_) => "<no reply>".to_string(),
    }
}

async fn boot<F>(configure: F) -> (TestServer, Socket)
where
    F: FnOnce(&mut ToniFactory),
{
    let mut factory = ToniFactory::new();
    configure(&mut factory);
    let server = TestServer::start_with(factory, GlobalsWsModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-globals", server.port);
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    (server, ws)
}

/// A guard the gateway never names runs on the connection and on every message,
/// ahead of the gateway's own.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_ws_guard_runs_ahead_of_the_gateway_s_own() {
    SEEN.lock().unwrap().clear();

    let (_server, mut ws) = boot(|f| {
        f.use_global_ws_guards(Arc::new(GlobalGuard));
    })
    .await;
    assert_eq!(ask(&mut ws, "ping").await, "pong");

    assert_eq!(
        seen(),
        vec![
            "global:guard:connect",
            "gateway:guard:connect",
            "global:guard:ping",
            "gateway:guard:ping",
            "handler",
        ]
    );
}

/// Refusing a message stops it before the gateway's own guard is asked, and
/// tells the caller nothing.
///
/// The silence is deliberate today — `toni_application`'s message callback
/// drops `WsError::AuthFailed` to keep the connection usable — and it is the one
/// transport that answers a rejection with nothing: HTTP sends 403, RPC a
/// `forbidden` frame, gRPC `PermissionDenied`. Pinned so the day it changes,
/// this says so.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_ws_guard_rejecting_stops_the_message() {
    SEEN.lock().unwrap().clear();

    let (_server, mut ws) = boot(|f| {
        f.use_global_ws_guards(Arc::new(DenyingGlobalGuard));
    })
    .await;

    assert_eq!(ask(&mut ws, "ping").await, "<no reply>");
    assert_eq!(
        seen(),
        vec![
            "global:deny:connect",
            "gateway:guard:connect",
            "global:deny:ping"
        ],
        "the connection is admitted, and the refused message never reaches the gateway's guard"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_ws_interceptor_wraps_every_handler() {
    SEEN.lock().unwrap().clear();

    let (_server, mut ws) = boot(|f| {
        f.use_global_ws_interceptors(Arc::new(GlobalInterceptor));
    })
    .await;
    assert_eq!(ask(&mut ws, "ping").await, "pong");

    assert_eq!(
        seen(),
        vec![
            "gateway:guard:connect",
            "gateway:guard:ping",
            "global:before",
            "handler",
            "global:after",
        ],
        "guards answer before the chain is entered, and the connect execution \
         runs guards without entering it at all"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_ws_error_handler_claims_what_the_gateway_leaves() {
    SEEN.lock().unwrap().clear();

    let (_server, mut ws) = boot(|f| {
        f.use_global_ws_error_handler(Arc::new(GlobalErrorHandler));
    })
    .await;
    assert_eq!(ask(&mut ws, "boom").await, "claimed globally");
    assert!(seen().contains(&"global:error_handler".to_string()));
}
