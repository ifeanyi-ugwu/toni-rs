//! Reconnect coverage for the RabbitMQ transport: after the broker connection
//! drops and comes back, the server's queues/consumers and the client's direct
//! reply-to consumer must be re-established, or RPC goes silent.
//!
//! The broker is paused (frozen via the cgroup freezer) rather than stopped —
//! stop/start reassigns the published host port, which already-connected
//! clients can't follow. A short `heartbeat` in the URI makes lapin notice the
//! frozen peer in a couple of seconds instead of the 60s default.
//!
//! Recovery relies on lapin's `enable_auto_recover`; without it the consumer
//! stream is cancelled on disconnect and never resumes. Gated behind the
//! `integration` feature (needs Docker).
#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::rabbitmq::RabbitMq;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, rpc_controller, RpcClient, ToniFactory};
use toni_rabbitmq::{RabbitMqAdapter, RabbitMqClientTransport};

static URI: OnceLock<String> = OnceLock::new();

#[rpc_controller(pub struct EchoController {})]
impl EchoController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("echo")]
    async fn echo(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(providers: [EchoController])]
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

#[tokio::test]
async fn rabbitmq_recovers_after_broker_freeze() {
    let container = RabbitMq::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5672).await.unwrap();
    // Short heartbeat so the frozen peer is detected in ~seconds, not ~60s.
    let uri = format!("amqp://guest:guest@127.0.0.1:{port}/%2f?heartbeat=2");
    URI.set(uri.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(EchoModule::module_definition())
                    .await;
                app.use_rpc_adapter(RabbitMqAdapter::new(URI.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                RabbitMqClientTransport::new(uri.clone()).with_timeout(Duration::from_secs(2)),
            );

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(10)).await,
                "echo should round-trip before the freeze"
            );

            // Freeze long enough to trip the 2s heartbeat on both ends, then thaw.
            container.pause().await.unwrap();
            tokio::time::sleep(Duration::from_secs(8)).await;
            container.unpause().await.unwrap();

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(25)).await,
                "echo should recover after the broker thaws (auto-recover replays topology)"
            );
        })
        .await;
}
