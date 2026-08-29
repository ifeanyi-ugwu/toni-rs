use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::SinkExt;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use tokio::sync::{mpsc, oneshot, OnceCell};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::{build_headers, header_str, HEADER_CORRELATION_ID};

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first reply carrying its id.
    Single(oneshot::Sender<Vec<u8>>),
    /// An `open_stream()` call: fed frame by frame until a terminal frame,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<ReplyFrame>),
}

type Pending = Arc<Mutex<HashMap<String, PendingSlot>>>;

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
                                let mut pending = router_pending.lock().unwrap();
                                match pending.get(&corr_id) {
                                    None => {}
                                    Some(PendingSlot::Single(_)) => {
                                        let Some(PendingSlot::Single(tx)) =
                                            pending.remove(&corr_id)
                                        else {
                                            unreachable!("slot variant checked above");
                                        };
                                        let _ = tx.send(payload);
                                    }
                                    Some(PendingSlot::Stream(tx)) => {
                                        let frame = wire::parse_reply_frame(&payload);
                                        let terminal = !matches!(frame, ReplyFrame::Item(_));
                                        let _ = tx.send(frame);
                                        if terminal {
                                            pending.remove(&corr_id);
                                        }
                                    }
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

        // The reply topic is unique to this client, so prefixing the counter
        // with it keeps correlation ids globally unique — the server's cancel
        // registry is shared by every caller.
        let corr_id = format!(
            "{}:{}",
            shared.reply_topic,
            shared.counter.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        shared
            .pending
            .lock()
            .unwrap()
            .insert(corr_id.clone(), PendingSlot::Single(tx));

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
            Ok(Ok(bytes)) => match wire::parse_reply_frame(&bytes) {
                ReplyFrame::Single(result) => result,
                ReplyFrame::Item(_) | ReplyFrame::End | ReplyFrame::EndErr { .. } => {
                    Err(RpcClientError::Transport(
                        "streaming reply to a single-reply call — use stream()".to_string(),
                    ))
                }
            },
            Ok(Err(_)) => Err(RpcClientError::Transport(
                "reply router stopped".to_string(),
            )),
            Err(_) => {
                shared.pending.lock().unwrap().remove(&corr_id);
                Err(RpcClientError::Timeout)
            }
        }
    }

    async fn open_stream(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcReplyStream, RpcClientError> {
        let shared = self.shared().await?;

        let corr_id = format!(
            "{}:{}",
            shared.reply_topic,
            shared.counter.fetch_add(1, Ordering::Relaxed)
        );
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        shared
            .pending
            .lock()
            .unwrap()
            .insert(corr_id.clone(), PendingSlot::Stream(raw_tx));

        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel::<()>();
        let (sink, stream) = RpcReplyStream::channel(32, move || {
            let _ = cancel_tx.send(());
        });
        tokio::spawn(forward_stream(
            raw_rx,
            sink,
            cancel_rx,
            self.timeout,
            shared.producer.clone(),
            shared.pending.clone(),
            corr_id.clone(),
        ));

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

        Ok(stream)
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

/// Feed one streaming call's frames from the reply router into the caller's
/// [`RpcReplyStream`], enforcing the per-frame gap deadline. A dropped stream
/// or an expired gap produces the cancel notice so the server stops
/// producing.
async fn forward_stream(
    mut raw_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut sink: ReplySink,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
    gap: Duration,
    producer: FutureProducer,
    pending: Pending,
    corr_id: String,
) {
    let publish_cancel = |producer: FutureProducer, corr_id: String| async move {
        let notice = wire::frame_cancel(&corr_id).to_string();
        let record = FutureRecord::to(crate::wire::CANCEL_TOPIC)
            .key(&corr_id)
            .payload(&notice);
        if let Err((e, _)) = producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
        {
            tracing::debug!(error = %e, "KafkaClientTransport cancel publish failed");
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                pending.lock().unwrap().remove(&corr_id);
                publish_cancel(producer.clone(), corr_id.clone()).await;
                break;
            }
            next = tokio::time::timeout(gap, raw_rx.recv()) => match next {
                Ok(Some(ReplyFrame::Item(data))) => {
                    let _ = sink.send(Ok(data)).await;
                }
                // A single-reply answer to a stream call: one item, then the
                // end.
                Ok(Some(ReplyFrame::Single(result))) => {
                    let _ = sink.send(result).await;
                    break;
                }
                Ok(Some(ReplyFrame::End)) => break,
                Ok(Some(ReplyFrame::EndErr { message, status })) => {
                    let _ = sink.send(Err(RpcClientError::Remote { message, status })).await;
                    break;
                }
                Ok(None) => {
                    let _ = sink
                        .send(Err(RpcClientError::Transport("reply router stopped".to_string())))
                        .await;
                    break;
                }
                Err(_) => {
                    let _ = sink.send(Err(RpcClientError::Timeout)).await;
                    pending.lock().unwrap().remove(&corr_id);
                    publish_cancel(producer.clone(), corr_id.clone()).await;
                    break;
                }
            }
        }
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
