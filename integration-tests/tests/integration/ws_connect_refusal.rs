//! A refused connection tells the caller why.
//!
//! Connect guards run after the handshake — there is no HTTP status left to
//! refuse with — so the refusal is answered the way RFC 6455 provides for:
//! the canonical envelope as a text frame, then a close carrying the code for
//! that refusal. A browser reads both off its `close` event.
//!
//! Before this, the adapter dropped the socket and the caller could not tell
//! a refusal from a crash or a dead network.

#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use toni::async_trait;
use toni::context::WsContext;
use toni::traits_helpers::Guard;
use toni::websocket::{WsHandlerResult, WsMessage};
use toni::{injectable, module};
use toni_macros::{new, subscribe_message, subscriptions, websocket_gateway};

use crate::common::TestServer;

#[injectable]
pub struct DenyConnect {}

#[async_trait]
impl Guard<WsContext> for DenyConnect {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        false
    }
}

#[injectable]
pub struct PanicOnConnect {}

#[async_trait]
impl Guard<WsContext> for PanicOnConnect {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        panic!("connect guard kaboom");
    }
}

#[websocket_gateway("/ws-refused")]
pub struct RefusedGateway {}

#[subscriptions]
#[use_guards(DenyConnect)]
impl RefusedGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }
}

#[module(providers: [DenyConnect, RefusedGateway])]
impl RefusedConnectModule {}

#[websocket_gateway("/ws-connect-panic")]
pub struct PanicConnectGateway {}

#[subscriptions]
#[use_guards(PanicOnConnect)]
impl PanicConnectGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }
}

#[module(providers: [PanicOnConnect, PanicConnectGateway])]
impl PanicConnectModule {}

/// Reads the envelope frame and the close frame a refusal answers with.
async fn refusal_of(url: &str) -> (serde_json::Value, u16) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("the handshake completes — guards run after it");

    let envelope = ws
        .next()
        .await
        .expect("a frame must arrive")
        .expect("the frame is readable");
    let envelope: serde_json::Value =
        serde_json::from_str(envelope.to_text().expect("a text frame")).expect("an envelope");

    let close = loop {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    let code = u16::from(close.expect("the close carries a frame").code);

    // Nothing is sent after the close.
    let _ = ws.close(None).await;
    (envelope, code)
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_refused_connection_is_told_the_policy() {
    let server = TestServer::start(RefusedConnectModule).await;
    let (envelope, code) = refusal_of(&format!("ws://127.0.0.1:{}/ws-refused", server.port)).await;

    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["kind"], "Unauthorized", "envelope: {envelope}");
    // 1008 Policy Violation — RFC 6455 has no auth-specific code.
    assert_eq!(code, 1008);
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_panicking_connect_guard_closes_as_a_server_fault() {
    let server = TestServer::start(PanicConnectModule).await;
    let (envelope, code) =
        refusal_of(&format!("ws://127.0.0.1:{}/ws-connect-panic", server.port)).await;

    // The panic keeps its type through the refusal, which is what separates a
    // server fault from a policy one on the wire.
    assert_eq!(envelope["kind"], "Internal", "envelope: {envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .unwrap_or_default()
            .contains("connect guard kaboom"),
        "envelope: {envelope}",
    );
    assert_eq!(code, 1011);
}
