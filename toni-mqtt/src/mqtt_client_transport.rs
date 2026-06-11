use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rumqttc::v5::mqttbytes::v5::{Packet, PublishProperties};
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::{AsyncClient, Event, MqttOptions};
use tokio::sync::{oneshot, OnceCell};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::wire::{data_to_bytes, parse_response};

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<u8>>>>>;

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
                                    let tx = router_pending.lock().unwrap().remove(&corr_id);
                                    if let Some(tx) = tx {
                                        let _ = tx.send(p.payload.to_vec());
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

        let corr_id = shared.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        shared.pending.lock().unwrap().insert(corr_id.clone(), tx);

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

fn client_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("toni-mqtt-client-{pid}-{nanos}")
}
