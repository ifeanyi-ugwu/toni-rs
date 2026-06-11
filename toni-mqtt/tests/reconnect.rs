//! Reconnect coverage for the MQTT v5 transport: after the broker connection
//! drops and comes back, both the server's handler subscriptions and the
//! client's reply subscription must be re-established, or RPC goes silent.
//!
//! The broker is paused (frozen via the cgroup freezer) rather than stopped —
//! stop/start reassigns the published host port, which the already-connected
//! clients can't follow. Pausing forces a keepalive-timeout disconnect on the
//! same port; unpausing lets the clients reconnect.
//!
//! Without the ConnAck-driven (re)subscribe, the post-reconnect `send` loop
//! never succeeds — rumqttc reconnects the socket but does not replay
//! subscriptions. Gated behind the `integration` feature (needs Docker).
#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, rpc_controller, RpcClient, ToniFactory};
use toni_mqtt::{MqttAdapter, MqttClientTransport};

static HOST: OnceLock<String> = OnceLock::new();
static PORT: OnceLock<u16> = OnceLock::new();

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

/// Drive `send("echo")` until it round-trips or the budget runs out.
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
async fn mqtt_recovers_after_broker_restart() {
    let container = Mosquitto::default().start().await.unwrap();
    let host = container.get_host().await.unwrap().to_string();
    let port = container.get_host_port_ipv4(1883).await.unwrap();
    HOST.set(host.clone()).ok();
    PORT.set(port).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(EchoModule::module_definition())
                    .await;
                app.use_rpc_adapter(MqttAdapter::new(
                    HOST.get().unwrap().clone(),
                    *PORT.get().unwrap(),
                ))
                .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                MqttClientTransport::new(host, port).with_timeout(Duration::from_secs(2)),
            );

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(8)).await,
                "echo should round-trip before the restart"
            );

            // Freeze the broker long enough to trip the 5s keepalive on both
            // ends (forcing a disconnect), then thaw it for the reconnect.
            container.pause().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
            container.unpause().await.unwrap();

            assert!(
                echo_succeeds_within(&client, Duration::from_secs(20)).await,
                "echo should recover after the broker restarts (resubscribe on reconnect)"
            );
        })
        .await;
}
