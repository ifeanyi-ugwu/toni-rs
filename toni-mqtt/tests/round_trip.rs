//! End-to-end coverage for the MQTT v5 RPC transport against a live broker
//! (testcontainers mosquitto). Gated behind the `integration` feature.
//!
//! - `send` round-trips a request via response_topic / correlation_data
//! - `emit` reaches a fire-and-forget handler with no response topic
//! - metadata set via `RpcClient::request().metadata(..)` rides v5 user
//!   properties and reaches the handler's `RpcContext`
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, new, patterns, rpc_controller, RpcClient, ToniFactory};
use toni_mqtt::{MqttAdapter, MqttClientTransport};

static HOST: OnceLock<String> = OnceLock::new();
static PORT: OnceLock<u16> = OnceLock::new();
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
async fn mqtt_rpc_send_emit_and_metadata() {
    let container = Mosquitto::default().start().await.unwrap();
    let host = container.get_host().await.unwrap().to_string();
    let port = container.get_host_port_ipv4(1883).await.unwrap();
    HOST.set(host.clone()).ok();
    PORT.set(port).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(MathModule::module_definition())
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
                MqttClientTransport::new(host.clone(), port).with_timeout(Duration::from_secs(2)),
            );

            // The server subscribes asynchronously after spawn; retry until the
            // subscription is live and the handler answers.
            let mut sum = None;
            for _ in 0..30u8 {
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

            // Metadata set via the client builder rides v5 user properties and
            // surfaces in the handler's RpcContext.
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
