//! Reconnect coverage for the Redis Pub/Sub transport: after the pubsub
//! connections are dropped, the server's channel subscriptions and the
//! client's reply psubscription must be re-established, or RPC goes silent.
//!
//! Redis pub/sub has no application-level heartbeat, so a frozen broker is
//! never noticed (the TCP socket stays open). To force a real disconnect the
//! test issues `CLIENT KILL` from a control connection, dropping the
//! transport's pubsub and normal connections server-side.
//!
//! Without the reconnect loops the post-kill `send` never recovers — the
//! pubsub stream ends and nothing resubscribes. Gated behind the `integration`
//! feature (needs Docker).
#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, new, patterns, rpc_controller, RpcClient, ToniFactory};
use toni_redis_rpc::{RedisAdapter, RedisClientTransport};

static URL: OnceLock<String> = OnceLock::new();

#[rpc_controller]
pub struct EchoController {}
#[patterns]
impl EchoController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("echo")]
    async fn echo(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [EchoController])]
impl EchoModule {}

async fn echo_succeeds_within(client: &RpcClient, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .send("echo", RpcData::json(serde_json::json!({"v": 1})))
            .await
        {
            if resp.as_json().and_then(|v| v["v"].as_i64()) == Some(1) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Drop every client connection server-side (SKIPME defaults to yes, so the
/// control connection itself survives).
async fn kill_all_connections(url: &str) {
    let client = redis::Client::open(url).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    for kind in ["pubsub", "normal"] {
        let _: i64 = redis::cmd("CLIENT")
            .arg("KILL")
            .arg("TYPE")
            .arg(kind)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);
    }
}

#[tokio::test]
async fn redis_recovers_after_connection_kill() {
    let container = Redis::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    URL.set(url.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new().create_with(EchoModule).await;
                app.use_rpc_adapter(RedisAdapter::new(URL.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                RedisClientTransport::new(url.clone()).with_timeout(Duration::from_secs(2)),
            );

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(8)).await,
                "echo should round-trip before the kill"
            );

            // Force a real disconnect on both sides.
            kill_all_connections(&url).await;

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(20)).await,
                "echo should recover after connections are killed (reconnect + resubscribe)"
            );
        })
        .await;
}
