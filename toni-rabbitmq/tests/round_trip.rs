//! End-to-end coverage for the RabbitMQ (AMQP) RPC transport against a live
//! broker (testcontainers). Gated behind the `integration` feature.
//!
//! - `send` round-trips a request via direct reply-to
//! - `emit` reaches a fire-and-forget handler with no reply queue
//! - metadata set via `RpcClient::request().metadata(..)` rides AMQP headers
//!   and reaches the handler's `RpcContext`
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::rabbitmq::RabbitMq;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::{controller, module, new, patterns, RpcClient, ToniFactory};
use toni_rabbitmq::{RabbitMqAdapter, RabbitMqClientTransport};

static URI: OnceLock<String> = OnceLock::new();
static EVENTS: AtomicUsize = AtomicUsize::new(0);

#[controller]
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
        let trace = c.header("trace").unwrap_or("none").to_string();
        Ok(RpcData::json(serde_json::json!({ "trace": trace })))
    }

    #[event_pattern("event.fire")]
    async fn fire(&self, _d: RpcData, _c: &RpcContext) -> Result<(), RpcError> {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(controllers: [MathController])]
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
                let mut app = ToniFactory::new().create_with(MathModule).await.unwrap();
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

            // Metadata set via the client builder rides AMQP headers and
            // surfaces in the handler's RpcContext.
            let resp = client
                .request("meta.echo")
                .header("trace", "abc123")
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

#[controller]
pub struct StreamController {}
#[patterns]
impl StreamController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("count.stream")]
    async fn count(&self, _d: RpcData) -> toni::rpc::RpcHandlerResult {
        use futures::StreamExt;
        Ok(toni::rpc::RpcHandlerOutput::Stream(
            futures::stream::iter((1..=3).map(|n| Ok(RpcData::json(serde_json::json!(n))))).boxed(),
        ))
    }

    #[message_pattern("probe.cancel")]
    async fn probe_cancel(&self, _d: RpcData, ctx: &RpcContext) -> toni::rpc::RpcHandlerResult {
        use futures::StreamExt;
        use toni::context::HandlerContext;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<RpcData, RpcError>>(1);
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            let mut n = 0u32;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        STREAM_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(30)) => {
                        n += 1;
                        if tx.send(Ok(RpcData::json(serde_json::json!(n)))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(toni::rpc::RpcHandlerOutput::Stream(
            tokio_stream::wrappers::ReceiverStream::new(rx).boxed(),
        ))
    }
}

#[module(controllers: [StreamController])]
impl StreamModule {}

static STREAM_URI: OnceLock<String> = OnceLock::new();
static STREAM_CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tokio::test]
async fn rabbitmq_rpc_streams_and_cancels() {
    use futures::StreamExt;

    let container = RabbitMq::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5672).await.unwrap();
    let uri = format!("amqp://127.0.0.1:{port}/%2f");
    STREAM_URI.set(uri.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new().create_with(StreamModule).await.unwrap();
                app.use_rpc_adapter(RabbitMqAdapter::new(STREAM_URI.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            let client = RpcClient::new(
                RabbitMqClientTransport::new(uri.clone()).with_timeout(Duration::from_secs(2)),
            );

            // The server declares its queues asynchronously after spawn;
            // probe with retries so a slow container does not flake the run.
            let mut items: Vec<i64> = Vec::new();
            for _ in 0..30u8 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Ok(stream) = client
                    .stream("count.stream", RpcData::json(serde_json::json!(null)))
                    .await
                {
                    items = stream
                        .filter_map(|item| async move {
                            item.ok().and_then(|d| d.as_json().and_then(|v| v.as_i64()))
                        })
                        .collect()
                        .await;
                    if !items.is_empty() {
                        break;
                    }
                }
            }
            assert_eq!(items, vec![1, 2, 3]);

            // Dropping the reply stream publishes the cancel notice; the
            // producer observes the execution's cancellation token.
            let mut stream = client
                .stream("probe.cancel", RpcData::json(serde_json::json!(null)))
                .await
                .unwrap();
            assert!(stream.next().await.is_some(), "first item");
            drop(stream);

            let mut cancelled = false;
            for _ in 0..40u8 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if STREAM_CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
            }
            assert!(cancelled, "producer never observed the cancellation token");
        })
        .await;
}
