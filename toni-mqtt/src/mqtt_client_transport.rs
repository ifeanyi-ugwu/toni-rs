use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::SinkExt;
use rumqttc::v5::mqttbytes::v5::{Packet, PublishProperties};
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::{AsyncClient, Event, MqttOptions};
use tokio::sync::{mpsc, oneshot, OnceCell};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::data_to_bytes;

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first reply carrying its id.
    Single(oneshot::Sender<Vec<u8>>),
    /// An `open_stream()` call: fed frame by frame until a terminal frame,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<ReplyFrame>),
}

type Pending = Arc<Mutex<HashMap<String, PendingSlot>>>;

/// MQTT v5 transport for [`RpcClient`].
///
/// Request-response uses the v5 `response_topic` / `correlation_data`
/// properties: every reply lands on one private topic this transport
/// subscribes to, and a correlation id routes each one back to the waiting
/// [`send`]. A single background task drives the rumqttc event loop, which is
/// what transmits queued publishes and delivers replies.
///
/// The connection is established lazily on first use, so the transport can be
/// built synchronously in a `provider_value!` block.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "INVENTORY_CLIENT",
///     toni::RpcClient::new(toni_mqtt::MqttClientTransport::new("127.0.0.1", 1883))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
/// [`send`]: MqttClientTransport::send
pub struct MqttClientTransport {
    host: String,
    port: u16,
    timeout: Duration,
    shared: OnceCell<Shared>,
}

struct Shared {
    client: AsyncClient,
    pending: Pending,
    counter: AtomicU64,
    reply_topic: String,
    poll: tokio::task::AbortHandle,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.poll.abort();
    }
}

impl MqttClientTransport {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
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
                let reply_topic = format!("toni/rpc/reply/{id}");

                let mut opts = MqttOptions::new(id, self.host.clone(), self.port);
                opts.set_keep_alive(Duration::from_secs(5));
                let (client, mut eventloop) = AsyncClient::new(opts, 64);

                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let router_pending = pending.clone();
                let router_client = client.clone();
                let router_reply_topic = reply_topic.clone();
                let (ready_tx, ready_rx) = oneshot::channel::<()>();

                let poll = tokio::spawn(async move {
                    let mut ready_tx = Some(ready_tx);
                    loop {
                        match eventloop.poll().await {
                            // Subscribe on every connect: rumqttc reconnects the
                            // socket after a drop but does not replay subscriptions,
                            // so the reply route must be re-established or replies
                            // stop arriving after a reconnect.
                            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                                if let Err(e) = router_client
                                    .subscribe(&router_reply_topic, QoS::AtLeastOnce)
                                    .await
                                {
                                    tracing::error!(error = %e, "MqttClientTransport failed to subscribe reply topic");
                                }
                            }
                            // Reply subscription is live — unblock the first send.
                            Ok(Event::Incoming(Packet::SubAck(_))) => {
                                if let Some(tx) = ready_tx.take() {
                                    let _ = tx.send(());
                                }
                            }
                            Ok(Event::Incoming(Packet::Publish(p))) => {
                                let corr = p.properties.and_then(|pr| pr.correlation_data);
                                if let Some(corr) = corr {
                                    let corr_id = String::from_utf8_lossy(&corr).to_string();
                                    let mut pending = router_pending.lock().unwrap();
                                    match pending.get(&corr_id) {
                                        None => {}
                                        Some(PendingSlot::Single(_)) => {
                                            let Some(PendingSlot::Single(tx)) =
                                                pending.remove(&corr_id)
                                            else {
                                                unreachable!("slot variant checked above");
                                            };
                                            let _ = tx.send(p.payload.to_vec());
                                        }
                                        Some(PendingSlot::Stream(tx)) => {
                                            let frame = wire::parse_reply_frame(&p.payload);
                                            let terminal =
                                                !matches!(frame, ReplyFrame::Item(_));
                                            let _ = tx.send(frame);
                                            if terminal {
                                                pending.remove(&corr_id);
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "MqttClientTransport connection error; retrying");
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                });

                // Best-effort: proceed even if the SubAck is slow; sends would
                // then rely on the caller retrying.
                let _ = tokio::time::timeout(Duration::from_secs(5), ready_rx).await;

                Ok(Shared {
                    client,
                    pending,
                    counter: AtomicU64::new(0),
                    reply_topic,
                    poll: poll.abort_handle(),
                })
            })
            .await
    }
}

#[async_trait]
impl RpcClientTransport for MqttClientTransport {
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

        let props = PublishProperties {
            response_topic: Some(shared.reply_topic.clone()),
            correlation_data: Some(corr_id.clone().into_bytes().into()),
            user_properties: metadata.into_iter().collect(),
            ..Default::default()
        };

        if let Err(e) = shared
            .client
            .publish_with_properties(pattern, QoS::AtLeastOnce, false, data_to_bytes(data), props)
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
            shared.client.clone(),
            shared.pending.clone(),
            corr_id.clone(),
        ));

        let props = PublishProperties {
            response_topic: Some(shared.reply_topic.clone()),
            correlation_data: Some(corr_id.clone().into_bytes().into()),
            user_properties: metadata.into_iter().collect(),
            ..Default::default()
        };

        if let Err(e) = shared
            .client
            .publish_with_properties(pattern, QoS::AtLeastOnce, false, data_to_bytes(data), props)
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
        let props = PublishProperties {
            user_properties: metadata.into_iter().collect(),
            ..Default::default()
        };
        shared
            .client
            .publish_with_properties(pattern, QoS::AtLeastOnce, false, data_to_bytes(data), props)
            .await
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
    client: AsyncClient,
    pending: Pending,
    corr_id: String,
) {
    let publish_cancel = |client: AsyncClient, corr_id: String| async move {
        let notice = wire::frame_cancel(&corr_id).to_string();
        if let Err(e) = client
            .publish(crate::wire::CANCEL_TOPIC, QoS::AtLeastOnce, false, notice)
            .await
        {
            tracing::debug!(error = %e, "MqttClientTransport cancel publish failed");
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                pending.lock().unwrap().remove(&corr_id);
                publish_cancel(client.clone(), corr_id.clone()).await;
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
                    publish_cancel(client.clone(), corr_id.clone()).await;
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
    format!("toni-mqtt-client-{pid}-{nanos}")
}
