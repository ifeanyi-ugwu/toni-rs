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
    build_headers, bytes_to_data, frame_panic, frame_response, header_str, metadata_from_headers,
    HEADER_CORRELATION_ID, HEADER_REPLY_TO,
};

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
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
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
            .expect("bind() must be called before into_lifecycle()");

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
            crate::wire::ensure_topics(&brokers, &patterns).await;

            let topics: Vec<&str> = patterns.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topics)
                .unwrap_or_else(|e| panic!("[KafkaAdapter] subscribe failed — {e}"));
            tracing::info!(?patterns, "KafkaAdapter subscribed");

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

                                tokio::spawn(handle_message(
                                    pattern, payload, reply_to, correlation_id, metadata,
                                    callbacks.clone(), producer.clone(),
                                ));
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

        Ok(toni::RpcLifecycleHandle::new(None, serve, move || async move {
            let _ = shutdown_tx.send(true);
            Ok(())
        }))
    }
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
    ctx.metadata = metadata;

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
        Ok(outcome) => frame_response(outcome),
        Err(_) => {
            tracing::error!("RPC handler panicked; returning error to caller");
            frame_panic()
        }
    };

    let headers = build_headers(None, correlation_id.as_deref(), &HashMap::new());
    let key = correlation_id.unwrap_or_default();
    let record = FutureRecord::to(&reply_to)
        .key(&key)
        .payload(&response)
        .headers(headers);

    if let Err((e, _)) = producer.send(record, Timeout::After(Duration::from_secs(5))).await {
        tracing::error!(error = %e, reply_to, "KafkaAdapter failed to publish reply");
    }
}
