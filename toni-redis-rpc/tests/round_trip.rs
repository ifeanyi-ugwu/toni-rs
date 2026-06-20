//! End-to-end coverage for the Redis Pub/Sub RPC transport against a live
//! Redis (testcontainers). Gated behind the `integration` feature because it
//! needs Docker.
//!
//! - `send` round-trips a request through the reply-router
//! - `emit` reaches a fire-and-forget handler with no reply channel
//! - metadata set via `RpcClient::request().metadata(..)` rides the request
//!   envelope and reaches the handler's `RpcContext`
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, new, patterns, rpc_controller, RpcClient, ToniFactory};
use toni_redis_rpc::{RedisAdapter, RedisClientTransport};

static URL: OnceLock<String> = OnceLock::new();
static EVENTS: AtomicUsize = AtomicUsize::new(0);

#[rpc_controller]
pub struct MathController {}
#[patterns]
impl MathController {
    #[new]
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
                    .send(
                        "math.add",
                        RpcData::json(serde_json::json!({"a": 2, "b": 3})),
                    )
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
            assert!(
                fired,
                "emit should reach the fire-and-forget handler exactly once"
            );

            // Metadata: set via the client builder must surface in the
            // handler's RpcContext and round-trip back.
            let resp = client
                .request("meta.echo")
                .metadata("trace", "abc123")
                .send(RpcData::json(serde_json::json!({})))
                .await
                .expect("metadata request should round-trip");
            let trace = resp.as_json().and_then(|v| v["trace"].as_str());
            assert_eq!(
                trace,
                Some("abc123"),
                "client metadata must reach the handler"
            );
        })
        .await;
}
