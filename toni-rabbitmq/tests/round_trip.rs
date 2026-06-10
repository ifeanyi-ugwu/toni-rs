//! End-to-end coverage for the RabbitMQ (AMQP) RPC transport against a live
//! broker (testcontainers). Gated behind the `integration` feature.
//!
//! - `send` round-trips a request via direct reply-to
//! - `emit` reaches a fire-and-forget handler with no reply queue
//! - AMQP headers on a request reach the handler's `RpcContext` metadata
//!   (a raw lapin publish stands in for a metadata-setting caller, since the
//!   client `send` API doesn't expose headers yet)
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::rabbitmq::RabbitMq;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{module, rpc_controller, RpcClient, ToniFactory};
use toni_rabbitmq::{RabbitMqAdapter, RabbitMqClientTransport};

static URI: OnceLock<String> = OnceLock::new();
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
async fn rabbitmq_rpc_send_emit_and_metadata() {
    let container = RabbitMq::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5672).await.unwrap();
    let uri = format!("amqp://guest:guest@127.0.0.1:{port}/%2f");
    URI.set(uri.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(MathModule::module_definition())
                    .await;
                app.use_rpc_adapter(RabbitMqAdapter::new(URI.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                RabbitMqClientTransport::new(uri.clone()).with_timeout(Duration::from_secs(2)),
            );

            // The server declares queues asynchronously after spawn; retry until
            // the handler queue exists and answers.
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

            let trace = raw_request_with_header(&uri, "meta.echo", "abc123").await;
            assert_eq!(trace, "abc123", "AMQP header metadata must reach the handler");
        })
        .await;
}

/// Publishes a request carrying an AMQP header and direct reply-to, returning
/// the handler's echoed `trace` header value.
async fn raw_request_with_header(uri: &str, pattern: &str, trace: &str) -> String {
    use futures::StreamExt;
    use lapin::options::{BasicConsumeOptions, BasicPublishOptions};
    use lapin::types::{AMQPValue, FieldTable};
    use lapin::{BasicProperties, Connection, ConnectionProperties};

    let conn = Connection::connect(uri, ConnectionProperties::default())
        .await
        .unwrap();
    let channel = conn.create_channel().await.unwrap();

    let mut consumer = channel
        .basic_consume(
            "amq.rabbitmq.reply-to".into(),
            "raw-reply".into(),
            BasicConsumeOptions {
                no_ack: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .unwrap();

    let mut headers = FieldTable::default();
    headers.insert("trace".into(), AMQPValue::LongString(trace.into()));

    channel
        .basic_publish(
            "".into(),
            pattern.into(),
            BasicPublishOptions::default(),
            b"{}",
            BasicProperties::default()
                .with_reply_to("amq.rabbitmq.reply-to".into())
                .with_correlation_id("raw-1".into())
                .with_headers(headers),
        )
        .await
        .unwrap();

    let delivery = tokio::time::timeout(Duration::from_secs(2), consumer.next())
        .await
        .expect("reply should arrive")
        .expect("stream should yield")
        .expect("delivery ok");
    let v: serde_json::Value = serde_json::from_slice(&delivery.data).unwrap();
    v["response"]["trace"].as_str().unwrap_or_default().to_string()
}
