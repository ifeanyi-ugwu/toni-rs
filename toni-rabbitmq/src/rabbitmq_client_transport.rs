use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use lapin::options::{BasicConsumeOptions, BasicPublishOptions, ExchangeDeclareOptions};
use lapin::types::{AMQPValue, FieldTable};
use lapin::{BasicProperties, Channel, Connection};
use tokio::sync::{mpsc, oneshot, OnceCell};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::data_to_bytes;

/// RabbitMQ direct reply-to pseudo-queue. Publishing with this as `reply_to`
/// tells the broker to route the reply straight back to this connection's
/// reply consumer — no real queue is declared.
const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first delivery carrying its id.
    Single(oneshot::Sender<Vec<u8>>),
    /// An `open_stream()` call: fed frame by frame until a terminal frame,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<ReplyFrame>),
}

type Pending = Arc<Mutex<HashMap<String, PendingSlot>>>;

/// RabbitMQ (AMQP) transport for [`RpcClient`].
///
/// Request-response uses RabbitMQ direct reply-to: a single consumer on
/// `amq.rabbitmq.reply-to` receives every reply, and a correlation id routes
/// each one back to the waiting [`send`]. The connection and consumer are
/// established lazily on first use, so the transport can be built synchronously
/// in a `provider_value!` block.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "INVENTORY_CLIENT",
///     toni::RpcClient::new(toni_rabbitmq::RabbitMqClientTransport::new("amqp://127.0.0.1:5672/%2f"))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
/// [`send`]: RabbitMqClientTransport::send
pub struct RabbitMqClientTransport {
    uri: String,
    timeout: Duration,
    shared: OnceCell<Shared>,
}

struct Shared {
    channel: Channel,
    pending: Pending,
    // Correlation ids are `{client_id}:{n}`: the cancel registry on the
    // server is shared by every caller, so a bare counter would collide
    // across clients and one caller's cancel could abort another's call.
    client_id: String,
    counter: AtomicU64,
    // Kept alive so the channel and reply consumer stay open; dropping the
    // connection closes both.
    _conn: Connection,
}

impl RabbitMqClientTransport {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
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
                // enable_auto_recover: lapin reconnects after a drop and replays
                // topology, re-establishing the direct reply-to consumer so
                // replies resume without manual re-setup.
                let props = lapin::ConnectionProperties::default().enable_auto_recover();
                let conn = Connection::connect(&self.uri, props)
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;
                let channel = conn
                    .create_channel()
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                // Direct reply-to requires a no-ack consumer on the pseudo-queue
                // to be active before any request that names it as reply_to.
                let consumer = channel
                    .basic_consume(
                        DIRECT_REPLY_TO.into(),
                        "toni-rabbitmq-reply".into(),
                        BasicConsumeOptions {
                            no_ack: true,
                            ..Default::default()
                        },
                        FieldTable::default(),
                    )
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                // The cancel exchange must exist before a stream's drop can
                // publish to it — publishing to a missing exchange closes the
                // channel. Declaring is idempotent.
                channel
                    .exchange_declare(
                        crate::wire::CANCEL_EXCHANGE.into(),
                        lapin::ExchangeKind::Fanout,
                        ExchangeDeclareOptions::default(),
                        FieldTable::default(),
                    )
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let router_pending = pending.clone();
                let mut consumer = consumer;
                tokio::spawn(async move {
                    while let Some(delivery) = consumer.next().await {
                        let Ok(delivery) = delivery else { continue };
                        let Some(corr) = delivery.properties.correlation_id().as_ref() else {
                            continue;
                        };
                        let mut pending = router_pending.lock().unwrap();
                        match pending.get(corr.as_str()) {
                            None => {}
                            Some(PendingSlot::Single(_)) => {
                                let Some(PendingSlot::Single(tx)) = pending.remove(corr.as_str())
                                else {
                                    unreachable!("slot variant checked above");
                                };
                                let _ = tx.send(delivery.data);
                            }
                            Some(PendingSlot::Stream(tx)) => {
                                let frame = wire::parse_reply_frame(&delivery.data);
                                let terminal = !matches!(frame, ReplyFrame::Item(_));
                                let _ = tx.send(frame);
                                if terminal {
                                    pending.remove(corr.as_str());
                                }
                            }
                        }
                    }
                });

                Ok(Shared {
                    channel,
                    pending,
                    client_id: make_client_id(),
                    counter: AtomicU64::new(0),
                    _conn: conn,
                })
            })
            .await
    }
}

#[async_trait]
impl RpcClientTransport for RabbitMqClientTransport {
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

        let props = BasicProperties::default()
            .with_reply_to(DIRECT_REPLY_TO.into())
            .with_correlation_id(corr_id.clone().into())
            .with_headers(metadata_to_headers(metadata));

        if let Err(e) = shared
            .channel
            .basic_publish(
                "".into(),
                pattern.into(),
                BasicPublishOptions::default(),
                &data_to_bytes(data),
                props,
            )
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
            shared.channel.clone(),
            shared.pending.clone(),
            corr_id.clone(),
        ));

        let props = BasicProperties::default()
            .with_reply_to(DIRECT_REPLY_TO.into())
            .with_correlation_id(corr_id.clone().into())
            .with_headers(metadata_to_headers(metadata));

        if let Err(e) = shared
            .channel
            .basic_publish(
                "".into(),
                pattern.into(),
                BasicPublishOptions::default(),
                &data_to_bytes(data),
                props,
            )
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
        let props = BasicProperties::default().with_headers(metadata_to_headers(metadata));
        shared
            .channel
            .basic_publish(
                "".into(),
                pattern.into(),
                BasicPublishOptions::default(),
                &data_to_bytes(data),
                props,
            )
            .await
            .map(|_| ())
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }
}

/// Feed one streaming call's frames from the reply router into the caller's
/// [`RpcReplyStream`], enforcing the per-frame gap deadline. A dropped stream
/// or an expired gap publishes the cancel notice so the server stops
/// producing.
async fn forward_stream(
    mut raw_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut sink: ReplySink,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
    gap: Duration,
    channel: Channel,
    pending: Pending,
    corr_id: String,
) {
    let publish_cancel = |channel: Channel, corr_id: String| async move {
        let notice = wire::frame_cancel(&corr_id).to_string();
        if let Err(e) = channel
            .basic_publish(
                crate::wire::CANCEL_EXCHANGE.into(),
                "".into(),
                BasicPublishOptions::default(),
                notice.as_bytes(),
                BasicProperties::default(),
            )
            .await
        {
            tracing::debug!(error = %e, "RabbitMqClientTransport cancel publish failed");
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                pending.lock().unwrap().remove(&corr_id);
                publish_cancel(channel.clone(), corr_id.clone()).await;
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
                    publish_cancel(channel.clone(), corr_id.clone()).await;
                    break;
                }
            }
        }
    }
}

fn make_client_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{pid}-{nanos}")
}

fn metadata_to_headers(metadata: HashMap<String, String>) -> FieldTable {
    let mut headers = FieldTable::default();
    for (key, value) in metadata {
        headers.insert(key.into(), AMQPValue::LongString(value.into()));
    }
    headers
}
