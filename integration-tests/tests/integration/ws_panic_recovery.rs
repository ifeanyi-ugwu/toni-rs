//! A panic inside a WebSocket handler is caught by the dispatcher,
//! surfaced as a `PanicRecovered` framework event, and rendered through
//! `WsError::to_message` — the connection stays alive and the next
//! message goes through normally. Sibling connections on the same gateway
//! are unaffected.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use toni::async_trait;
use toni::context::WsContext;
use toni::errors::{ErrorKind, PanicRecovered, PipelineSegment};
use toni::injectable;
use toni::module;
use toni::traits_helpers::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use toni::websocket::{WsClient, WsError, WsHandlerResult, WsMessage};
use toni_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;

/// Start an Axum-backed app with the supplied global WS error handlers wired
/// before bootstrap.
async fn start_ws_server_with_handlers(
    module: impl toni::ModuleMetadata + 'static,
    handlers: Vec<Arc<dyn ErrorHandler<WsContext, WsMessage>>>,
) -> u16 {
    use toni::toni_factory::ToniFactory;
    use toni_axum::AxumAdapter;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        for h in handlers {
            factory.use_global_ws_error_handler(h);
        }
        let mut app = factory.create_with(module).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.http.expect("HTTP adapter not bound").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });
    port_rx.await.unwrap()
}

/// Global chain handler that records the `PanicRecovered.during` it is
/// handed and declines, so the default rendering still answers the client.
struct WsSegmentRecorder {
    count: Arc<AtomicUsize>,
    captured: Arc<std::sync::Mutex<Option<PipelineSegment>>>,
}

#[async_trait]
impl ErrorHandler<WsContext, WsMessage> for WsSegmentRecorder {
    async fn handle_error(&self, error: ChainError<'_>, _ctx: &WsContext) -> Option<WsMessage> {
        self.count.fetch_add(1, Ordering::SeqCst);
        if let Some(p) = error.downcast_ref::<PanicRecovered>() {
            *self.captured.lock().unwrap() = Some(p.during);
        }
        None
    }
}

#[websocket_gateway("/ws-panic-recovery")]
pub struct PanicGateway {}
#[subscriptions]
impl PanicGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("panic")]
    async fn on_panic(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        panic!("intentional test panic");
    }

    #[subscribe_message("safe")]
    async fn on_safe(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("safe-ok").into())
    }
}

#[module(providers: [PanicGateway])]
impl PanicGatewayModule {}

/// A handler panic surfaces as a canonical-envelope WS frame; the connection
/// stays open and the same client can send another message. Sibling
/// connections are unaffected.
///
/// Note: the test produces a "panicked at" line in stderr — that is the
/// Rust panic hook firing before catch_unwind catches the unwind. It is
/// expected and does not indicate a test failure.
#[tokio_localset_test::localset_test]
async fn ws_handler_panic_renders_envelope_and_keeps_connection_alive() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(PanicGatewayModule).await;
    let ws_url = format!("ws://127.0.0.1:{}/ws-panic-recovery", server.port);

    // Client A triggers the panic — receives the canonical envelope, not a Close.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws_a.send(Message::Text(r#"{"event":"panic"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws_a.next().await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or_default()
            .contains("intentional test panic"),
        "panic message should surface in the envelope, got: {json}",
    );

    // Same connection still works for subsequent messages.
    ws_a.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws_a.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "safe-ok");

    // Client B (connected after the panic) reaches the safe handler normally.
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws_b.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws_b.next().await.unwrap().unwrap();
    assert_eq!(
        reply.to_text().unwrap(),
        "safe-ok",
        "sibling connections must be unaffected by another client's handler panic"
    );
}

#[injectable]
pub struct PanickingWsGuard {}
impl PanickingWsGuard {}

#[async_trait]
impl Guard<WsContext> for PanickingWsGuard {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        panic!("ws guard kaboom");
    }
}

#[websocket_gateway("/ws-guard-panic")]
pub struct WsGuardPanicGateway {}
#[subscriptions]
impl WsGuardPanicGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("guarded")]
    #[use_guards(PanickingWsGuard)]
    async fn on_guarded(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }

    #[subscribe_message("safe")]
    async fn on_safe(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("safe-ok").into())
    }
}

#[module(providers: [PanickingWsGuard, WsGuardPanicGateway])]
impl WsGuardPanicModule {}

#[tokio_localset_test::localset_test]
async fn ws_guard_panic_renders_envelope_and_keeps_connection_alive() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let recorder = Arc::new(WsSegmentRecorder {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_ws_server_with_handlers(WsGuardPanicModule, vec![recorder]).await;

    let url = format!("ws://127.0.0.1:{}/ws-guard-panic", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(r#"{"event":"guarded"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Guard));

    ws.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "safe-ok");
}

#[injectable]
pub struct PanickingWsInterceptor {}
impl PanickingWsInterceptor {}

#[async_trait]
impl Interceptor<WsContext, WsHandlerResult> for PanickingWsInterceptor {
    async fn intercept(
        &self,
        _ctx: &WsContext,
        _next: Box<dyn InterceptorNext<WsContext, WsHandlerResult>>,
    ) -> WsHandlerResult {
        panic!("ws interceptor kaboom");
    }
}

#[websocket_gateway("/ws-interceptor-panic")]
pub struct WsInterceptorPanicGateway {}
#[subscriptions]
impl WsInterceptorPanicGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("intercepted")]
    #[use_interceptors(PanickingWsInterceptor)]
    async fn on_intercepted(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }

    #[subscribe_message("safe")]
    async fn on_safe(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("safe-ok").into())
    }
}

#[module(providers: [PanickingWsInterceptor, WsInterceptorPanicGateway])]
impl WsInterceptorPanicModule {}

#[tokio_localset_test::localset_test]
async fn ws_interceptor_panic_renders_envelope_and_keeps_connection_alive() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let recorder = Arc::new(WsSegmentRecorder {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_ws_server_with_handlers(WsInterceptorPanicModule, vec![recorder]).await;

    let url = format!("ws://127.0.0.1:{}/ws-interceptor-panic", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(
        r#"{"event":"intercepted"}"#.to_string().into(),
    ))
    .await
    .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Middleware));

    ws.send(Message::Text(r#"{"event":"safe"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "safe-ok");
}

#[injectable]
pub struct PanickingWsErrorHandler {}
impl PanickingWsErrorHandler {}

#[async_trait]
impl ErrorHandler<WsContext, WsMessage> for PanickingWsErrorHandler {
    async fn handle_error(&self, _error: ChainError<'_>, _ctx: &WsContext) -> Option<WsMessage> {
        panic!("ws error-handler kaboom");
    }
}

#[websocket_gateway("/ws-eh-panic")]
pub struct WsErrorHandlerPanicGateway {}
#[subscriptions]
impl WsErrorHandlerPanicGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("eh")]
    #[use_error_handlers(PanickingWsErrorHandler)]
    async fn on_eh(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        panic!("handler kaboom");
    }
}

#[module(providers: [PanickingWsErrorHandler, WsErrorHandlerPanicGateway])]
impl WsErrorHandlerPanicModule {}

#[tokio_localset_test::localset_test]
async fn ws_error_handler_panic_continues_chain_to_default_rendering() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let recorder = Arc::new(WsSegmentRecorder {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_ws_server_with_handlers(WsErrorHandlerPanicModule, vec![recorder]).await;

    let url = format!("ws://127.0.0.1:{}/ws-eh-panic", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(r#"{"event":"eh"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    // Chain skipped the panicking handler and fell through to the
    // default rendering, which surfaces the original handler panic.
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");

    // The chain survived the panicking handler: the global recorder that sits
    // behind it still saw the original handler panic. The panic inside the
    // handler itself reaches no handler — it is logged and nothing else.
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        *captured.lock().unwrap(),
        Some(PipelineSegment::HandlerBody)
    );
}

/// Domain error whose `message()` panics — exercises the renderer-panic
/// fallback path of the WebSocket dispatcher.
#[derive(Debug)]
struct WsRenderBomb;

impl std::fmt::Display for WsRenderBomb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WsRenderBomb")
    }
}

impl std::error::Error for WsRenderBomb {}

impl toni::Error for WsRenderBomb {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }
    fn message(&self) -> std::borrow::Cow<'_, str> {
        panic!("ws render kaboom");
    }
}

#[websocket_gateway("/ws-render-panic")]
pub struct WsRenderPanicGateway {}
#[subscriptions]
impl WsRenderPanicGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("render")]
    async fn on_render(&self, _c: WsClient, _m: WsMessage) -> WsHandlerResult {
        Err(WsError::from(WsRenderBomb))
    }
}

#[module(providers: [WsRenderPanicGateway])]
impl WsRenderPanicModule {}

#[tokio_localset_test::localset_test]
async fn ws_renderer_panic_falls_back_to_safe_envelope() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let server = TestServer::start(WsRenderPanicModule).await;

    let url = format!("ws://127.0.0.1:{}/ws-render-panic", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(r#"{"event":"render"}"#.to_string().into()))
        .await
        .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    // The renderer runs below the chain, so its panic is logged and nothing
    // else — this frame is the only signal the client gets, and it carries the
    // canonical envelope rather than a bare string.
    let json: serde_json::Value =
        serde_json::from_str(reply.to_text().unwrap()).expect("the fallback must be JSON");
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "Internal");
    assert_eq!(json["message"], "Internal Server Error", "frame: {json}");
}
