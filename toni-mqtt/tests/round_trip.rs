//! End-to-end coverage for the MQTT v5 RPC transport against a live broker
//! (testcontainers mosquitto). Gated behind the `integration` feature.
//!
//! - `send` round-trips a request via response_topic / correlation_data
//! - `emit` reaches a fire-and-forget handler with no response topic
//! - MQTT v5 user properties on a request reach the handler's `RpcContext`
//!   metadata (a raw rumqttc publish stands in for a metadata-setting caller,
//!   since the client `send` API doesn't expose user properties yet)
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
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
            for _ in 0..20u8 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if EVENTS.load(Ordering::SeqCst) == 1 {
                    fired = true;
                    break;
                }
            }
            assert!(fired, "emit should reach the fire-and-forget handler exactly once");

            let trace = raw_request_with_user_property(&host, port, "meta.echo", "abc123").await;
            assert_eq!(trace, "abc123", "v5 user property must reach the handler");
        })
        .await;
}

/// Publishes a request carrying a v5 user property and response_topic/
/// correlation_data, returning the handler's echoed `trace`.
async fn raw_request_with_user_property(host: &str, port: u16, pattern: &str, trace: &str) -> String {
    use rumqttc::v5::mqttbytes::v5::{Packet, PublishProperties};
    use rumqttc::v5::mqttbytes::QoS;
    use rumqttc::v5::{AsyncClient, Event, MqttOptions};

    let mut opts = MqttOptions::new("toni-mqtt-raw-test", host, port);
    opts.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(opts, 16);

    let reply_topic = "toni/rpc/reply/raw-test";
    client.subscribe(reply_topic, QoS::AtLeastOnce).await.unwrap();

    // Drive the loop until the subscription is acked before publishing.
    loop {
        if let Ok(Event::Incoming(Packet::SubAck(_))) = eventloop.poll().await {
            break;
        }
    }

    let props = PublishProperties {
        response_topic: Some(reply_topic.to_string()),
        correlation_data: Some("raw-1".as_bytes().to_vec().into()),
        user_properties: vec![("trace".to_string(), trace.to_string())],
        ..Default::default()
    };
    client
        .publish_with_properties(pattern, QoS::AtLeastOnce, false, b"{}".to_vec(), props)
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return "timeout".to_string();
        }
        if let Ok(Event::Incoming(Packet::Publish(p))) = eventloop.poll().await {
            let v: serde_json::Value = serde_json::from_slice(&p.payload).unwrap();
            return v["response"]["trace"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }
}
