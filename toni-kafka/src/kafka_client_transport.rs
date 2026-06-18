use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use tokio::sync::{oneshot, OnceCell};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::{build_headers, header_str, parse_response, HEADER_CORRELATION_ID};

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<u8>>>>>;

/// Apache Kafka transport for [`RpcClient`].
///
/// Request-response is emulated over the log: each [`send`] produces a request
/// carrying a private reply-topic and a correlation id in its headers, and one
/// background consumer on that reply topic routes each reply back to the
/// waiting call. The reply consumer reads from `earliest` so a reply produced
/// before partition assignment completes is still delivered.
///
/// Established lazily on first use, so the transport can be built synchronously
/// in a `provider_value!` block.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "INVENTORY_CLIENT",
///     toni::RpcClient::new(toni_kafka::KafkaClientTransport::new("127.0.0.1:9092"))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
/// [`send`]: KafkaClientTransport::send
pub struct KafkaClientTransport {
    brokers: String,
    timeout: Duration,
    shared: OnceCell<Shared>,
}

struct Shared {
    producer: FutureProducer,
    pending: Pending,
    counter: AtomicU64,
    reply_topic: String,
    router: tokio::task::AbortHandle,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.router.abort();
    }
}

impl KafkaClientTransport {
    pub fn new(brokers: impl Into<String>) -> Self {
        Self {
            brokers: brokers.into(),
            timeout: Duration::from_secs(5),
            shared: OnceCell::new(),
        }
    }

    /// Override the request-response timeout (default: 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn shared(&self) -> Result<&Shared, RpcClientError> {
        self.shared
            .get_or_try_init(|| async {
                let id = client_id();
                let reply_topic = format!("toni.rpc.reply.{id}");

                let producer: FutureProducer = ClientConfig::new()
                    .set("bootstrap.servers", &self.brokers)
                    .create()
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                // A unique group per transport instance so it reads every reply
                // on its private topic; `earliest` avoids losing a reply that
                // lands before partition assignment finishes.
                let consumer: StreamConsumer = ClientConfig::new()
                    .set("bootstrap.servers", &self.brokers)
                    .set("group.id", &id)
                    .set("auto.offset.reset", "earliest")
                    .set("enable.auto.commit", "true")
                    .create()
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;
                // Create the reply topic up front so the consumer assigns its
                // partition immediately and no reply is missed at startup.
                crate::wire::ensure_topics(&self.brokers, std::slice::from_ref(&reply_topic)).await;
                consumer
                    .subscribe(&[reply_topic.as_str()])
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let router_pending = pending.clone();
                let router = tokio::spawn(async move {
                    loop {
                        match consumer.recv().await {
                            Ok(msg) => {
                                let Some(corr_id) =
                                    header_str(msg.headers(), HEADER_CORRELATION_ID)
                                else {
                                    continue;
                                };
                                let payload = msg.payload().map(|p| p.to_vec()).unwrap_or_default();
                                let tx = router_pending.lock().unwrap().remove(&corr_id);
                                if let Some(tx) = tx {
                                    let _ = tx.send(payload);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "KafkaClientTransport reply recv error");
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                });

                Ok(Shared {
                    producer,
                    pending,
                    counter: AtomicU64::new(0),
                    reply_topic,
                    router: router.abort_handle(),
                })
            })
            .await
    }
}

#[async_trait]
impl RpcClientTransport for KafkaClientTransport {
    async fn connect(&self) -> Result<(), RpcClientError> {
        self.shared().await?;
        Ok(())
    }

    async fn send(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcData, RpcClientError> {
        let shared = self.shared().await?;

        let corr_id = shared.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        shared.pending.lock().unwrap().insert(corr_id.clone(), tx);

        let payload = crate::wire::data_to_bytes(data);
        let headers = build_headers(Some(&shared.reply_topic), Some(&corr_id), &metadata);
        let record = FutureRecord::to(pattern)
            .key(&corr_id)
            .payload(&payload)
            .headers(headers);

        if let Err((e, _)) = shared
            .producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
        {
            shared.pending.lock().unwrap().remove(&corr_id);
            return Err(RpcClientError::Transport(e.to_string()));
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(bytes)) => parse_response(&bytes),
            Ok(Err(_)) => Err(RpcClientError::Transport(
                "reply router stopped".to_string(),
            )),
            Err(_) => {
                shared.pending.lock().unwrap().remove(&corr_id);
                Err(RpcClientError::Timeout)
            }
        }
    }

    async fn emit(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<(), RpcClientError> {
        let shared = self.shared().await?;

        let key = shared.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let payload = crate::wire::data_to_bytes(data);
        let headers = build_headers(None, None, &metadata);
        let record = FutureRecord::to(pattern)
            .key(&key)
            .payload(&payload)
            .headers(headers);

        shared
            .producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
            .map(|_| ())
            .map_err(|(e, _)| RpcClientError::Transport(e.to_string()))
    }
}

fn client_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("toni-kafka-client-{pid}-{nanos}")
}
