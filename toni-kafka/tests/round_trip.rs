//! End-to-end coverage for the Kafka RPC transport against a live broker
//! (testcontainers). Gated behind the `integration` feature.
//!
//! - `send` round-trips a request via a private reply topic + correlation id
//! - `emit` reaches a fire-and-forget handler with no reply topic
//! - metadata set via `RpcClient::request().metadata(..)` rides Kafka headers
//!   and reaches the handler's `RpcContext`
//!
//! Topics auto-create on the broker. Budgets are generous: a Kafka broker boots
//! slowly and consumer-group assignment adds several seconds before the first
//! request is consumed.
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, rpc_controller, RpcClient, ToniFactory};
use toni_kafka::{KafkaAdapter, KafkaClientTransport};

static BROKERS: OnceLock<String> = OnceLock::new();
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
async fn kafka_rpc_send_emit_and_metadata() {
    let container = Kafka::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(KAFKA_PORT).await.unwrap();
    let brokers = format!("127.0.0.1:{port}");
    BROKERS.set(brokers.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(MathModule::module_definition())
                    .await;
                app.use_rpc_adapter(KafkaAdapter::new(BROKERS.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                KafkaClientTransport::new(brokers.clone()).with_timeout(Duration::from_secs(5)),
            );

            // Broker boot + consumer-group assignment can take many seconds;
            // retry until the handler topic is assigned and answers.
            let mut sum = None;
            for _ in 0..60u8 {
                tokio::time::sleep(Duration::from_millis(1000)).await;
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

            EVENTS.store(0, Ordering::SeqCst);
            client
                .emit("event.fire", RpcData::json(serde_json::json!({})))
                .await
                .unwrap();
            let mut fired = false;
            for _ in 0..30u8 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if EVENTS.load(Ordering::SeqCst) == 1 {
                    fired = true;
                    break;
                }
            }
            assert!(fired, "emit should reach the fire-and-forget handler exactly once");

            let resp = client
                .request("meta.echo")
                .metadata("trace", "abc123")
                .send(RpcData::json(serde_json::json!({})))
                .await
                .expect("metadata request should round-trip");
            let trace = resp.as_json().and_then(|v| v["trace"].as_str());
            assert_eq!(trace, Some("abc123"), "client metadata must reach the handler");
        })
        .await;
}
