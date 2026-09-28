use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::wire::{HEADER_CORRELATION_ID, build_headers, header_str};
use futures::SinkExt;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use tokio::sync::{OnceCell, mpsc, oneshot};
use ulo::async_trait;
use ulo::rpc::wire::{self, ReplyFrame};
use ulo::rpc::{ReplySink, RpcReplyStream};
use ulo::rpc::{RpcClientError, RpcClientTransport, RpcData};

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
/// as a provider value.
///
/// # Example
///
/// ```ignore
/// key!(pub Inventory: ulo::rpc::RpcClient);
///
/// provide!(Inventory =>
///     ulo::rpc::RpcClient::new(ulo_rpc_kafka::KafkaClientTransport::new("127.0.0.1:9092"))
/// )
/// ```
///
/// [`RpcClient`]: ulo::rpc::RpcClient
/// [`send`]: KafkaClientTransport::send
pub struct KafkaClientTransport {
    brokers: String,
    timeout: Duration,
    reply_topic: Option<String>,
    topic_shape: crate::wire::TopicShape,
    shared: OnceCell<Shared>,
}

struct Shared {
    producer: FutureProducer,
    pending: Pending,
    counter: AtomicU64,
    reply_topic: String,
    /// Prefixes every correlation id. Per transport instance even when the reply topic is named,
    /// so a restarted client never reuses a dead one's ids.
    client_id: String,
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
            reply_topic: None,
            topic_shape: crate::wire::TopicShape::default(),
            shared: OnceCell::new(),
        }
    }

    /// Bound each publish and each wait for a reply (default: 5 s). A `send` publishes and waits
    /// for its reply within this; an `emit`, a stream's opening publish and its cancel notice are
    /// each bounded by it. A stream also takes it as the longest gap between frames.
    ///
    /// A publish that runs out stays in rdkafka's queue and may still be delivered, so a call that
    /// answered `Timeout` can still reach the server. The topic setup on a client's first use,
    /// when [`connect`](RpcClientTransport::connect) was not called first, is not bounded by this.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Name the reply topic, and the consumer group that reads it, so a
    /// restarted client reuses one topic on the cluster instead of leaving one
    /// behind per start. Unset, each transport names both after its process id
    /// and the time it first connects. Correlation ids stay per transport
    /// either way. One live transport per name: transports sharing it share one
    /// consumer group, which gives each reply partition to only one of them,
    /// and a call whose reply reaches another times out.
    pub fn with_reply_topic(mut self, name: impl Into<String>) -> Self {
        self.reply_topic = Some(name.into());
        self
    }

    /// Partitions for the reply topic this client creates (default 1). A topic
    /// that already exists keeps its shape.
    pub fn with_topic_partitions(mut self, partitions: i32) -> Self {
        self.topic_shape.partitions = partitions;
        self
    }

    /// Replication factor for the reply topic this client creates (default 1).
    /// A topic that already exists keeps its shape.
    pub fn with_replication_factor(mut self, replication: i32) -> Self {
        self.topic_shape.replication = replication;
        self
    }

    async fn shared(&self) -> Result<&Shared, RpcClientError> {
        self.shared
            .get_or_try_init(|| async {
                let client_id = client_id();
                let (id, reply_topic) = match &self.reply_topic {
                    Some(name) => (name.clone(), name.clone()),
                    None => (client_id.clone(), format!("ulo.rpc.reply.{client_id}")),
                };

                let producer: FutureProducer = ClientConfig::new()
                    .set("bootstrap.servers", &self.brokers)
                    .create()
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                // One group per reply topic, so this transport reads every reply
                // on its private topic; `earliest` avoids losing a reply that
                // arrives before partition assignment finishes.
                let consumer: StreamConsumer = ClientConfig::new()
                    .set("bootstrap.servers", &self.brokers)
                    .set("group.id", &id)
                    .set("auto.offset.reset", "earliest")
                    .set("enable.auto.commit", "true")
                    .create()
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;
                // Create the reply topic up front so the consumer assigns its
                // partition immediately and no reply is missed at startup.
                crate::wire::ensure_topics(
                    &self.brokers,
                    std::slice::from_ref(&reply_topic),
                    self.topic_shape,
                )
                .await;
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
                    client_id,
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

        // The client id is unique to this transport instance, so prefixing the
        // counter with it keeps correlation ids globally unique — the server's
        // cancel registry is shared by every caller.
        let corr_id = format!(
            "{}:{}",
            shared.client_id,
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

        // One deadline for the whole call: the publish and the reply share `self.timeout`, so a
        // slow publish shortens the wait for the reply rather than adding to it.
        let deadline = tokio::time::Instant::now() + self.timeout;
        match tokio::time::timeout_at(
            deadline,
            shared.producer.send(record, Timeout::After(self.timeout)),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err((e, _))) => {
                shared.pending.lock().unwrap().remove(&corr_id);
                return Err(RpcClientError::Transport(e.to_string()));
            }
            Err(_) => {
                shared.pending.lock().unwrap().remove(&corr_id);
                return Err(RpcClientError::Timeout);
            }
        }

        match tokio::time::timeout_at(deadline, rx).await {
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
            shared.client_id,
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

        match publish(&shared.producer, record, self.timeout).await {
            Ok(()) => Ok(stream),
            Err(e) => {
                shared.pending.lock().unwrap().remove(&corr_id);
                Err(e)
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

        publish(&shared.producer, record, self.timeout).await
    }
}

/// Publish `record`, the delivery wait included, within `bound`.
///
/// rdkafka's own timeout covers only queueing: with the broker gone, the delivery future waits on
/// `message.timeout.ms`, far past any bound a caller set. So the whole future runs under `bound`,
/// and running out of it answers `Timeout`.
async fn publish<K, P>(
    producer: &FutureProducer,
    record: FutureRecord<'_, K, P>,
    bound: Duration,
) -> Result<(), RpcClientError>
where
    K: rdkafka::message::ToBytes + ?Sized,
    P: rdkafka::message::ToBytes + ?Sized,
{
    match tokio::time::timeout(bound, producer.send(record, Timeout::After(bound))).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err((e, _))) => Err(RpcClientError::Transport(e.to_string())),
        Err(_) => Err(RpcClientError::Timeout),
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
        if let Err(e) = publish(&producer, record, gap).await {
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
    format!("ulo-kafka-client-{pid}-{nanos}")
}
