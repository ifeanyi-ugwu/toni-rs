use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use toni::{RpcAdapter, RpcCallInfo, RpcMessageCallbacks};

use crate::wire::{
    build_headers, bytes_to_data, header_str, metadata_from_headers, HEADER_CORRELATION_ID,
    HEADER_REPLY_TO,
};
use toni::rpc::wire::{frame_panic, frame_response};

/// Apache Kafka transport adapter for the Toni RPC gateway.
///
/// A `StreamConsumer` (in a stable consumer group) subscribes to one topic per
/// registered pattern. A request carrying a `toni-reply-to` header gets its
/// reply produced to that topic with the `toni-correlation-id` echoed back; a
/// request without one is fire-and-forget. Headers other than the two control
/// keys surface as the handler's `RpcContext` metadata.
///
/// `auto.offset.reset` is `latest`: a freshly started group processes new
/// requests rather than replaying the topic's history.
///
/// # Example
///
/// ```ignore
/// app.use_rpc_adapter(toni_kafka::KafkaAdapter::new("127.0.0.1:9092")).unwrap();
/// ```
pub struct KafkaAdapter {
    brokers: String,
    group_id: String,
    patterns: Vec<String>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl KafkaAdapter {
    pub fn new(brokers: impl Into<String>) -> Self {
        Self {
            brokers: brokers.into(),
            group_id: "toni-rpc-server".to_string(),
            patterns: Vec::new(),
            callbacks: None,
        }
    }

    /// Override the consumer group id (default `toni-rpc-server`). Instances
    /// sharing a group load-balance the pattern topics' partitions.
    pub fn with_group_id(mut self, group_id: impl Into<String>) -> Self {
        self.group_id = group_id.into();
        self
    }
}

#[toni::async_trait]
impl RpcAdapter for KafkaAdapter {
    fn register_handlers(
        &mut self,
        patterns: &[String],
        callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<()> {
        self.patterns = patterns.to_vec();
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::RpcLifecycleHandle> {
        let brokers = self.brokers.clone();
        let group_id = self.group_id.clone();
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let consumer: StreamConsumer = ClientConfig::new()
                .set("bootstrap.servers", &brokers)
                .set("group.id", &group_id)
                .set("auto.offset.reset", "latest")
                .set("enable.auto.commit", "true")
                .create()
                .unwrap_or_else(|e| panic!("[KafkaAdapter] consumer create failed — {e}"));

            let producer: FutureProducer = ClientConfig::new()
                .set("bootstrap.servers", &brokers)
                .create()
                .unwrap_or_else(|e| panic!("[KafkaAdapter] producer create failed — {e}"));

            // Create the handler topics up front so the subscribe below assigns
            // their partitions immediately rather than after a metadata refresh.
            let mut created = patterns.clone();
            created.push(crate::wire::CANCEL_TOPIC.to_string());
            crate::wire::ensure_topics(&brokers, &created).await;

            let topics: Vec<&str> = patterns.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topics)
                .unwrap_or_else(|e| panic!("[KafkaAdapter] subscribe failed — {e}"));
            tracing::info!(?patterns, "KafkaAdapter subscribed");

            // Streaming calls in flight, keyed by correlation id, abortable
            // by a cancel notice. The cancel consumer runs in a unique
            // per-instance group: instances in the pattern group share the
            // request partitions, but every instance must see every notice
            // because only the one holding the call can act on it.
            let inflight_calls = toni::rpc::wire::Inflight::new();
            {
                let cancel_consumer: StreamConsumer = ClientConfig::new()
                    .set("bootstrap.servers", &brokers)
                    .set("group.id", format!("toni-rpc-cancel-{}", instance_id()))
                    .set("auto.offset.reset", "latest")
                    .set("enable.auto.commit", "true")
                    .create()
                    .unwrap_or_else(|e| {
                        panic!("[KafkaAdapter] cancel consumer create failed — {e}")
                    });
                cancel_consumer
                    .subscribe(&[crate::wire::CANCEL_TOPIC])
                    .unwrap_or_else(|e| panic!("[KafkaAdapter] cancel subscribe failed — {e}"));
                let inflight_calls = inflight_calls.clone();
                tokio::spawn(async move {
                    loop {
                        match cancel_consumer.recv().await {
                            Ok(msg) => {
                                let payload = msg.payload().unwrap_or_default();
                                let Ok(v) = serde_json::from_slice::<serde_json::Value>(payload)
                                else {
                                    continue;
                                };
                                if v.get("cancel").and_then(|c| c.as_bool()) == Some(true) {
                                    if let Some(key) = v["key"].as_str() {
                                        inflight_calls.cancel(key);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "KafkaAdapter cancel recv error; retrying");
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                });
            }

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    received = consumer.recv() => {
                        match received {
                            Ok(msg) => {
                                let pattern = msg.topic().to_string();
                                let payload = msg.payload().map(|p| p.to_vec()).unwrap_or_default();
                                let reply_to = header_str(msg.headers(), HEADER_REPLY_TO);
                                let correlation_id = header_str(msg.headers(), HEADER_CORRELATION_ID);
                                let metadata = metadata_from_headers(msg.headers());

                                // Register a request-shaped call before dispatch
                                // so a cancel notice can abort it mid-handler or
                                // mid-drain. A notice racing this registration is
                                // dropped — the cancel channel is best-effort on
                                // a broker.
                                let corr_key = reply_to
                                    .as_ref()
                                    .and(correlation_id.as_ref())
                                    .cloned();
                                let (abort_slot, guard) = match corr_key {
                                    Some(key) => {
                                        let abort_slot = Arc::new(std::sync::Mutex::new(
                                            None::<tokio::task::AbortHandle>,
                                        ));
                                        let slot = abort_slot.clone();
                                        let guard = inflight_calls.register(key, move || {
                                            if let Some(handle) = slot.lock().unwrap().take() {
                                                handle.abort();
                                            }
                                        });
                                        (Some(abort_slot), Some(guard))
                                    }
                                    None => (None, None),
                                };

                                let callbacks = callbacks.clone();
                                let producer = producer.clone();
                                let handle = tokio::spawn(async move {
                                    let _guard = guard;
                                    handle_message(
                                        pattern, payload, reply_to, correlation_id, metadata,
                                        callbacks, producer,
                                    )
                                    .await;
                                });
                                if let Some(slot) = abort_slot {
                                    *slot.lock().unwrap() = Some(handle.abort_handle());
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "KafkaAdapter recv error; retrying");
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                }
            }
        });

        Ok(toni::RpcLifecycleHandle::new(
            None,
            serve,
            move || async move {
                let _ = shutdown_tx.send(true);
                Ok(())
            },
        ))
    }
}

/// Names this instance's cancel consumer group uniquely, so two instances on
/// one broker never share one.
fn instance_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{pid}-{nanos}")
}

#[allow(clippy::too_many_arguments)]
async fn handle_message(
    pattern: String,
    payload: Vec<u8>,
    reply_to: Option<String>,
    correlation_id: Option<String>,
    metadata: HashMap<String, String>,
    callbacks: Arc<RpcMessageCallbacks>,
    producer: FutureProducer,
) {
    let data = bytes_to_data(&payload);
    let mut ctx = RpcCallInfo::new(pattern);
    ctx.headers = metadata;

    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
        .catch_unwind()
        .await;

    let Some(reply_to) = reply_to else {
        if outcome.is_err() {
            tracing::error!("RPC handler panicked on fire-and-forget message");
        }
        return;
    };

    let response = match outcome {
        Ok(Ok(toni::RpcHandlerOutput::Stream(stream))) => {
            toni::rpc::wire::drive_reply_stream(stream, |frame| {
                let producer = producer.clone();
                let reply_to = reply_to.clone();
                let correlation_id = correlation_id.clone();
                async move {
                    let headers = build_headers(None, correlation_id.as_deref(), &HashMap::new());
                    // Keyed by correlation id: every frame of one call lands
                    // on one partition, which is what keeps them ordered.
                    let key = correlation_id.unwrap_or_default();
                    let payload = frame.to_string();
                    let record = FutureRecord::to(&reply_to)
                        .key(&key)
                        .payload(&payload)
                        .headers(headers);
                    producer
                        .send(record, Timeout::After(Duration::from_secs(5)))
                        .await
                        .map(|_| ())
                        .map_err(|(e, _)| {
                            tracing::error!(error = %e, reply_to, "KafkaAdapter stream publish error");
                        })
                }
            })
            .await;
            return;
        }
        Ok(outcome) => frame_response(outcome).into_bytes(),
        Err(_) => {
            tracing::error!("RPC handler panicked; returning error to caller");
            frame_panic().into_bytes()
        }
    };

    let headers = build_headers(None, correlation_id.as_deref(), &HashMap::new());
    let key = correlation_id.unwrap_or_default();
    let record = FutureRecord::to(&reply_to)
        .key(&key)
        .payload(&response)
        .headers(headers);

    if let Err((e, _)) = producer
        .send(record, Timeout::After(Duration::from_secs(5)))
        .await
    {
        tracing::error!(error = %e, reply_to, "KafkaAdapter failed to publish reply");
    }
}
