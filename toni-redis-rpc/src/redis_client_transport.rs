use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, OnceCell};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::RequestEnvelope;

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first reply on its channel.
    Single(oneshot::Sender<Vec<u8>>),
    /// An `open_stream()` call: fed frame by frame until a terminal frame,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<ReplyFrame>),
}

type Pending = Arc<Mutex<HashMap<String, PendingSlot>>>;

/// Redis Pub/Sub transport for [`RpcClient`].
///
/// Redis Pub/Sub gives a publisher no way to receive a reply, so request-
/// response is emulated: each [`send`] publishes a reply-channel name with the
/// request and waits for the server to publish back to it. A single background
/// router subscribes — once, with a wildcard — to every reply channel this
/// transport will ever use, so there is no per-request connection churn.
///
/// The connection and router are established lazily on first use, so the
/// transport can be constructed synchronously inside a `provider_value!` block.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "INVENTORY_CLIENT",
///     toni::RpcClient::new(toni_redis_rpc::RedisClientTransport::new("redis://127.0.0.1:6379"))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
/// [`send`]: RedisClientTransport::send
pub struct RedisClientTransport {
    url: String,
    timeout: Duration,
    shared: OnceCell<Shared>,
}

struct Shared {
    publisher: redis::aio::ConnectionManager,
    pending: Pending,
    client_id: String,
    counter: AtomicU64,
    router: tokio::task::AbortHandle,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.router.abort();
    }
}

impl RedisClientTransport {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
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
                let client = redis::Client::open(self.url.as_str())
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;
                // ConnectionManager reconnects publishes on its own; only the
                // reply pubsub needs hand-rolled reconnect (see the router).
                let publisher = client
                    .get_connection_manager()
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;
                let mut pubsub = client
                    .get_async_pubsub()
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                let client_id = make_client_id();
                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let reply_pattern = format!("toni:rpc:reply:{client_id}:*");

                // Wildcard-subscribe before any send publishes, so a reply can
                // never arrive ahead of the subscription that routes it.
                pubsub
                    .psubscribe(&reply_pattern)
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))?;

                let router_pending = pending.clone();
                let router_client = client.clone();
                let router = tokio::spawn(async move {
                    let mut pubsub = pubsub;
                    loop {
                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            let channel = msg.get_channel_name();
                            let Some(corr_id) = channel.rsplit(':').next() else {
                                continue;
                            };
                            let payload = msg.get_payload::<Vec<u8>>().unwrap_or_default();
                            let mut pending = router_pending.lock().unwrap();
                            match pending.get(corr_id) {
                                None => {}
                                Some(PendingSlot::Single(_)) => {
                                    let Some(PendingSlot::Single(tx)) = pending.remove(corr_id)
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
                                        pending.remove(corr_id);
                                    }
                                }
                            }
                        }

                        // Reply connection dropped — Redis pubsub has no
                        // auto-recovery, so reopen and re-psubscribe before
                        // resuming, or replies would stop routing.
                        drop(stream);
                        tracing::warn!("RedisClientTransport reply stream ended; reconnecting");
                        pubsub = loop {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            match router_client.get_async_pubsub().await {
                                Ok(mut ps) => match ps.psubscribe(&reply_pattern).await {
                                    Ok(()) => break ps,
                                    Err(e) => tracing::warn!(error = %e, "reply re-psubscribe failed; retrying"),
                                },
                                Err(e) => tracing::warn!(error = %e, "reply reconnect failed; retrying"),
                            }
                        };
                    }
                });

                Ok(Shared {
                    publisher,
                    pending,
                    client_id,
                    counter: AtomicU64::new(0),
                    router: router.abort_handle(),
                })
            })
            .await
    }
}

#[async_trait]
impl RpcClientTransport for RedisClientTransport {
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
        let reply_to = format!("toni:rpc:reply:{}:{}", shared.client_id, corr_id);

        let (tx, rx) = oneshot::channel();
        shared
            .pending
            .lock()
            .unwrap()
            .insert(corr_id.clone(), PendingSlot::Single(tx));

        let envelope = RequestEnvelope {
            data,
            reply_to: Some(reply_to),
            metadata,
        };
        let payload =
            serde_json::to_vec(&envelope).map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let mut conn = shared.publisher.clone();
        if let Err(e) = redis::cmd("PUBLISH")
            .arg(pattern)
            .arg(payload)
            .query_async::<()>(&mut conn)
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
            // Router task is gone — the reply can never arrive.
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

        let corr_id = shared.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let reply_to = format!("toni:rpc:reply:{}:{}", shared.client_id, corr_id);

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
            shared.publisher.clone(),
            shared.pending.clone(),
            corr_id.clone(),
            reply_to.clone(),
        ));

        let envelope = RequestEnvelope {
            data,
            reply_to: Some(reply_to),
            metadata,
        };
        let payload =
            serde_json::to_vec(&envelope).map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let mut conn = shared.publisher.clone();
        if let Err(e) = redis::cmd("PUBLISH")
            .arg(pattern)
            .arg(payload)
            .query_async::<()>(&mut conn)
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

        let envelope = RequestEnvelope {
            data,
            reply_to: None,
            metadata,
        };
        let payload =
            serde_json::to_vec(&envelope).map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let mut conn = shared.publisher.clone();
        redis::cmd("PUBLISH")
            .arg(pattern)
            .arg(payload)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }
}

/// Feed one streaming call's frames from the reply router into the caller's
/// [`RpcReplyStream`], enforcing the per-frame gap deadline. A dropped stream
/// or an expired gap publishes the cancel notice so the server stops
/// producing.
#[allow(clippy::too_many_arguments)]
async fn forward_stream(
    mut raw_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut sink: ReplySink,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
    gap: Duration,
    publisher: redis::aio::ConnectionManager,
    pending: Pending,
    corr_id: String,
    reply_to: String,
) {
    let publish_cancel = |mut publisher: redis::aio::ConnectionManager, reply_to: String| async move {
        let notice = wire::frame_cancel(&reply_to).to_string();
        if let Err(e) = redis::cmd("PUBLISH")
            .arg(crate::wire::CANCEL_CHANNEL)
            .arg(notice.into_bytes())
            .query_async::<()>(&mut publisher)
            .await
        {
            tracing::debug!(error = %e, "RedisClientTransport cancel publish failed");
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                pending.lock().unwrap().remove(&corr_id);
                publish_cancel(publisher.clone(), reply_to.clone()).await;
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
                    publish_cancel(publisher.clone(), reply_to.clone()).await;
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
