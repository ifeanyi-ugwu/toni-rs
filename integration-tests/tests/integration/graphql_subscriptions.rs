//! Integration tests for GraphQL subscriptions via the graphql-ws protocol.
//!
//! Uses a real Axum HTTP server with WebSocket upgrade. The schema exposes a single
//! `countdown(from: Int!)` subscription that emits integers from `from` down to 0.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use serde_json::Value;
use toni::module;
use toni::{async_trait, WsClient};
use toni_async_graphql::{
    async_graphql::{self, Context, EmptyMutation, Object, Schema, Subscription},
    DefaultContextBuilder, GraphQLModule, SubscriptionContextBuilder,
};
use tokio_tungstenite::tungstenite::Message;

use crate::common::TestServer;

// ---- Schema --------------------------------------------------------------

struct Query;

#[Object]
impl Query {
    async fn ping(&self) -> &str {
        "pong"
    }
}

struct Sub;

#[Subscription]
impl Sub {
    async fn countdown(&self, from: i32) -> impl futures_util::Stream<Item = i32> {
        futures_util::stream::iter((0..=from).rev())
    }
}

// ---- Module --------------------------------------------------------------

fn build_module() -> GraphQLModule<Query, EmptyMutation, Sub, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, Sub).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder)
        .with_path("/graphql")
        .with_subscription_path("/graphql/ws")
}

#[module(imports: [build_module()], controllers: [], providers: [], exports: [])]
impl GqlModule {}

// ---- Auth-payload schema -------------------------------------------------

struct AuthQuery;

#[Object]
impl AuthQuery {
    async fn ping(&self) -> &str {
        "pong"
    }
}

struct AuthSub;

#[Subscription]
impl AuthSub {
    /// Emits the token that was passed in connection_init's payload.
    async fn auth_token(
        &self,
        ctx: &Context<'_>,
    ) -> impl futures_util::Stream<Item = String> {
        let token = ctx
            .data::<String>()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "<missing>".to_owned());
        futures_util::stream::once(std::future::ready(token))
    }
}

struct TokenContextBuilder;

#[async_trait]
impl SubscriptionContextBuilder for TokenContextBuilder {
    async fn build(
        &self,
        _client: &WsClient,
        init_payload: Option<Value>,
    ) -> async_graphql::Data {
        let mut data = async_graphql::Data::default();
        if let Some(token) = init_payload
            .as_ref()
            .and_then(|p| p.get("token"))
            .and_then(|v| v.as_str())
        {
            data.insert(token.to_owned());
        }
        data
    }
}

fn build_auth_module(
) -> GraphQLModule<AuthQuery, EmptyMutation, AuthSub, DefaultContextBuilder> {
    let schema = Schema::build(AuthQuery, EmptyMutation, AuthSub).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder)
        .with_path("/graphql")
        .with_subscription_path("/graphql/ws")
        .with_subscription_context(TokenContextBuilder)
}

#[module(imports: [build_auth_module()], controllers: [], providers: [], exports: [])]
impl AuthModule {}

// ---- Helpers -------------------------------------------------------------

async fn connect_ws(port: u16) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    connect_ws_with_protocol(port, Some("graphql-transport-ws")).await
}

async fn connect_ws_with_protocol(
    port: u16,
    protocol: Option<&str>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let url = format!("ws://127.0.0.1:{}/graphql/ws", port);
    let mut req = url.into_client_request().unwrap();
    if let Some(p) = protocol {
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            p.parse().unwrap(),
        );
    }
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

fn text(s: impl Into<String>) -> Message {
    Message::Text(s.into().into())
}

/// Collect up to `n` text frames within `timeout`, ignoring non-text frames.
async fn collect_n(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    n: usize,
    timeout: Duration,
) -> Vec<Value> {
    let mut out = Vec::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    out.push(serde_json::from_str(&t).unwrap());
                    if out.len() == n { break; }
                }
                Some(Ok(_)) => {}
                _ => break,
            }
        }
    }
    out
}

// ---- Tests ---------------------------------------------------------------

/// A `connection_init` is acknowledged with `connection_ack`.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_connection_ack() {
    let server = TestServer::start(GqlModule::module_definition()).await;
    let mut ws = connect_ws(server.port).await;

    ws.send(text(r#"{"type":"connection_init"}"#)).await.unwrap();
    let msgs = collect_n(&mut ws, 1, Duration::from_secs(2)).await;

    assert_eq!(msgs[0]["type"], "connection_ack");
}

/// A subscription emits all `next` frames followed by a `complete` frame.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_subscription_delivers_items_and_complete() {
    let server = TestServer::start(GqlModule::module_definition()).await;
    let mut ws = connect_ws(server.port).await;

    // Handshake
    ws.send(text(r#"{"type":"connection_init"}"#)).await.unwrap();
    collect_n(&mut ws, 1, Duration::from_secs(2)).await; // consume ack

    // Subscribe: countdown from 3 → [3, 2, 1, 0] + complete = 5 frames
    ws.send(text(
        r#"{"type":"subscribe","id":"1","payload":{"query":"subscription { countdown(from: 3) }"}}"#,
    ))
    .await
    .unwrap();

    let frames = collect_n(&mut ws, 5, Duration::from_secs(5)).await;

    // First four are "next" frames with data
    let next_values: Vec<i64> = frames[..4]
        .iter()
        .map(|f| {
            assert_eq!(f["type"], "next");
            assert_eq!(f["id"], "1");
            f["payload"]["data"]["countdown"].as_i64().unwrap()
        })
        .collect();

    assert_eq!(next_values, [3, 2, 1, 0]);

    // Fifth is the "complete" sentinel
    assert_eq!(frames[4]["type"], "complete");
    assert_eq!(frames[4]["id"], "1");
}

/// A ping is answered with a pong.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_ping_pong() {
    let server = TestServer::start(GqlModule::module_definition()).await;
    let mut ws = connect_ws(server.port).await;

    ws.send(text(r#"{"type":"connection_init"}"#)).await.unwrap();
    collect_n(&mut ws, 1, Duration::from_secs(2)).await;

    ws.send(text(r#"{"type":"ping"}"#)).await.unwrap();
    let msgs = collect_n(&mut ws, 1, Duration::from_secs(2)).await;

    assert_eq!(msgs[0]["type"], "pong");
}

/// Connections without `Sec-WebSocket-Protocol: graphql-transport-ws` are rejected.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_rejects_missing_subprotocol() {
    let server = TestServer::start(GqlModule::module_definition()).await;
    let mut ws = connect_ws_with_protocol(server.port, None).await;

    // Server closes the connection immediately — no ack should arrive.
    let _ = ws.send(text(r#"{"type":"connection_init"}"#)).await;
    let msgs = collect_n(&mut ws, 1, Duration::from_millis(500)).await;

    assert!(msgs.is_empty());
}

/// Connections with a wrong sub-protocol value are also rejected.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_rejects_wrong_subprotocol() {
    let server = TestServer::start(GqlModule::module_definition()).await;
    let mut ws = connect_ws_with_protocol(server.port, Some("graphql-ws")).await;

    let _ = ws.send(text(r#"{"type":"connection_init"}"#)).await;
    let msgs = collect_n(&mut ws, 1, Duration::from_millis(500)).await;

    assert!(msgs.is_empty());
}

/// The `connection_init` payload is forwarded to the context builder and available
/// in subscription resolvers via `ctx.data::<T>()`.
#[serial]
#[tokio_localset_test::localset_test]
async fn graphql_ws_connection_init_payload_reaches_resolver() {
    let server = TestServer::start(AuthModule::module_definition()).await;
    let mut ws = connect_ws(server.port).await;

    // Handshake with an auth token in the payload
    ws.send(text(
        r#"{"type":"connection_init","payload":{"token":"secret123"}}"#,
    ))
    .await
    .unwrap();
    collect_n(&mut ws, 1, Duration::from_secs(2)).await; // consume ack

    // Subscribe to authToken — expects 1 next + 1 complete
    ws.send(text(
        r#"{"type":"subscribe","id":"1","payload":{"query":"subscription { authToken }"}}"#,
    ))
    .await
    .unwrap();

    let frames = collect_n(&mut ws, 2, Duration::from_secs(5)).await;

    assert_eq!(frames[0]["type"], "next");
    assert_eq!(frames[0]["payload"]["data"]["authToken"], "secret123");
    assert_eq!(frames[1]["type"], "complete");
}
