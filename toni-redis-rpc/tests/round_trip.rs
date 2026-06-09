//! End-to-end coverage for the Redis Pub/Sub RPC transport against a live
//! Redis (testcontainers). Gated behind the `integration` feature because it
//! needs Docker.
//!
//! - `send` round-trips a request through the reply-router
//! - `emit` reaches a fire-and-forget handler with no reply channel
//! - metadata carried in the request envelope reaches the handler's
//!   `RpcContext` (a raw publish stands in for a metadata-setting caller,
//!   since the client `send` API doesn't expose metadata yet)
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, rpc_controller, RpcClient, ToniFactory};
use toni_redis_rpc::{RedisAdapter, RedisClientTransport};

static URL: OnceLock<String> = OnceLock::new();
static EVENTS: AtomicUsize = AtomicUsize::new(0);

#[rpc_controller(pub struct MathController {})]
impl MathController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("math.add")]
    async fn add(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        let v = data.as_json().cloned().unwrap_or_default();
        let a = v["a"].as_i64().unwrap_or(0);
        let b = v["b"].as_i64().unwrap_or(0);
        Ok(RpcData::json(serde_json::json!({ "sum": a + b })))
    }

    #[message_pattern("meta.echo")]
    async fn meta_echo(&self, _d: RpcData, c: &RpcContext) -> Result<RpcData, RpcError> {
        let trace = c.get_metadata("trace").unwrap_or("none").to_string();
        Ok(RpcData::json(serde_json::json!({ "trace": trace })))
    }

    #[event_pattern("event.fire")]
    async fn fire(&self, _d: RpcData, _c: &RpcContext) -> Result<(), RpcError> {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(providers: [MathController])]
impl MathModule {}

#[tokio::test]
async fn redis_rpc_send_emit_and_metadata() {
    let container = Redis::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    URL.set(url.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(MathModule::module_definition())
                    .await;
                app.use_rpc_adapter(RedisAdapter::new(URL.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            // The server subscribes asynchronously after spawn; give it a beat,
            // then probe with retries so a slow container doesn't flake the run.
            let client = RpcClient::new(
                RedisClientTransport::new(url.clone()).with_timeout(Duration::from_secs(2)),
            );

            let mut sum = None;
            for _ in 0..20u8 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Ok(resp) = client
                    .send("math.add", RpcData::json(serde_json::json!({"a": 2, "b": 3})))
                    .await
                {
                    sum = resp.as_json().and_then(|v| v["sum"].as_i64());
                    if sum.is_some() {
                        break;
                    }
                }
            }
            assert_eq!(sum, Some(5), "send round-trip should return the sum");

            // Fire-and-forget: no reply, but the handler must run.
            EVENTS.store(0, Ordering::SeqCst);
            client
                .emit("event.fire", RpcData::json(serde_json::json!({})))
                .await
                .unwrap();
            let mut fired = false;
            for _ in 0..20u8 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if EVENTS.load(Ordering::SeqCst) == 1 {
                    fired = true;
                    break;
                }
            }
            assert!(fired, "emit should reach the fire-and-forget handler exactly once");

            // Metadata: a raw publish carrying envelope metadata must surface in
            // the handler's RpcContext.
            let trace = raw_request_with_metadata(&url, "meta.echo", "abc123").await;
            assert_eq!(trace, "abc123", "envelope metadata must reach the handler");
        })
        .await;
}

/// Publishes a hand-built request envelope (with `metadata`) and waits for the
/// reply on a one-off subscription, returning the handler's echoed `trace`.
async fn raw_request_with_metadata(url: &str, pattern: &str, trace: &str) -> String {
    use futures::StreamExt;

    let client = redis::Client::open(url).unwrap();
    let mut publisher = client.get_multiplexed_async_connection().await.unwrap();
    let mut pubsub = client.get_async_pubsub().await.unwrap();

    let reply_to = "test:raw:reply";
    pubsub.subscribe(reply_to).await.unwrap();

    let envelope = serde_json::json!({
        "data": { "Json": {} },
        "reply_to": reply_to,
        "metadata": { "trace": trace },
    });
    redis::cmd("PUBLISH")
        .arg(pattern)
        .arg(envelope.to_string())
        .query_async::<()>(&mut publisher)
        .await
        .unwrap();

    let mut stream = pubsub.on_message();
    let msg = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("reply should arrive")
        .expect("stream should yield a message");
    let payload = msg.get_payload::<String>().unwrap();
    let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
    v["response"]["trace"].as_str().unwrap_or_default().to_string()
}
