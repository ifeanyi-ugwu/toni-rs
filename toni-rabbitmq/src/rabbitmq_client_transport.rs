use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use lapin::options::{BasicConsumeOptions, BasicPublishOptions};
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection};
use tokio::sync::{oneshot, OnceCell};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::{data_to_bytes, parse_response};

/// RabbitMQ direct reply-to pseudo-queue. Publishing with this as `reply_to`
/// tells the broker to route the reply straight back to this connection's
/// reply consumer — no real queue is declared.
const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<u8>>>>>;

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

                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let router_pending = pending.clone();
                let mut consumer = consumer;
                tokio::spawn(async move {
                    while let Some(delivery) = consumer.next().await {
                        let Ok(delivery) = delivery else { continue };
                        let Some(corr) = delivery.properties.correlation_id().as_ref() else {
                            continue;
                        };
                        let tx = router_pending.lock().unwrap().remove(corr.as_str());
                        if let Some(tx) = tx {
                            let _ = tx.send(delivery.data);
                        }
                    }
                });

                Ok(Shared {
                    channel,
                    pending,
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

    async fn send(&self, pattern: &str, data: RpcData) -> Result<RpcData, RpcClientError> {
        let shared = self.shared().await?;

        let corr_id = shared.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        shared.pending.lock().unwrap().insert(corr_id.clone(), tx);

        let props = BasicProperties::default()
            .with_reply_to(DIRECT_REPLY_TO.into())
            .with_correlation_id(corr_id.clone().into());

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

    async fn emit(&self, pattern: &str, data: RpcData) -> Result<(), RpcClientError> {
        let shared = self.shared().await?;
        shared
            .channel
            .basic_publish(
                "".into(),
                pattern.into(),
                BasicPublishOptions::default(),
                &data_to_bytes(data),
                BasicProperties::default(),
            )
            .await
            .map(|_| ())
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }
}
