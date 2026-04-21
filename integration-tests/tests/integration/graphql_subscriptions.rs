//! Integration tests for GraphQL subscriptions via the graphql-ws protocol.
//!
//! Uses a real Axum HTTP server with WebSocket upgrade. The schema exposes a single
//! `countdown(from: Int!)` subscription that emits integers from `from` down to 0.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use serde_json::Value;
use toni::module;
use toni_async_graphql::{
    async_graphql::{self, EmptyMutation, Object, Schema, Subscription},
    DefaultContextBuilder, GraphQLModule,
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

// ---- Helpers -------------------------------------------------------------

async fn connect_ws(port: u16) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let url = format!("ws://127.0.0.1:{}/graphql/ws", port);
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
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
