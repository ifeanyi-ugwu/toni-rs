//! End-to-end coverage for the NATS RPC transport against a live server
//! (testcontainers). Gated behind the `integration` feature because it needs
//! Docker.
//!
//! - `send` round-trips a request through the reply inbox
//! - a streaming handler's items arrive in order and the stream ends
//! - dropping the client's reply stream publishes the cancel notice and the
//!   producer observes the execution's cancellation token
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::nats::Nats;
use toni::context::{HandlerContext, RpcContext};
use toni::rpc::{RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};
use toni::{controller, module, new, patterns, RpcClient, ToniFactory};
use toni_nats::{NatsAdapter, NatsClientTransport};

static URL: OnceLock<String> = OnceLock::new();
static PRODUCER_SAW_CANCEL: AtomicBool = AtomicBool::new(false);

#[controller]
pub struct StreamController {}
#[patterns]
impl StreamController {
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

    #[message_pattern("count.stream")]
    async fn count(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures::stream::iter((1..=3).map(|n| Ok(RpcData::json(serde_json::json!(n))))).boxed(),
        ))
    }

    #[message_pattern("probe.cancel")]
    async fn probe_cancel(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<RpcData, RpcError>>(1);
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            let mut n = 0u32;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        PRODUCER_SAW_CANCEL.store(true, Ordering::SeqCst);
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
        Ok(RpcHandlerOutput::Stream(
            tokio_stream::wrappers::ReceiverStream::new(rx).boxed(),
        ))
    }
}

#[module(controllers: [StreamController])]
impl StreamModule {}

#[tokio::test]
async fn nats_rpc_streams_and_cancels() {
    let container = Nats::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let url = format!("nats://127.0.0.1:{port}");
    URL.set(url.clone()).ok();

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new().create_with(StreamModule).await.unwrap();
                app.use_rpc_adapter(NatsAdapter::new(URL.get().unwrap().clone()))
                    .unwrap();
                app.bind().await.unwrap();
                app.run().await;
            });

            // The server subscribes asynchronously after spawn; probe with
            // retries so a slow container does not flake the run.
            let client = RpcClient::new(
                NatsClientTransport::new(url.clone()).with_timeout(Duration::from_secs(2)),
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

            // Streamed items arrive in order and the stream ends cleanly.
            let stream = client
                .stream("count.stream", RpcData::json(serde_json::json!(null)))
                .await
                .unwrap();
            let items: Vec<i64> = stream
                .map(|item| item.unwrap().as_json().and_then(|v| v.as_i64()).unwrap())
                .collect()
                .await;
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
                if PRODUCER_SAW_CANCEL.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
            }
            assert!(cancelled, "producer never observed the cancellation token");
        })
        .await;
}
