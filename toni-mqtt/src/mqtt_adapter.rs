use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use rumqttc::v5::mqttbytes::v5::{Packet, Publish, PublishProperties};
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::{AsyncClient, Event, MqttOptions};
use toni::{RpcAdapter, RpcCallInfo, RpcMessageCallbacks};

use crate::wire::{bytes_to_data, frame_panic, frame_response, user_properties_to_metadata};

/// MQTT v5 transport adapter for the Toni RPC gateway.
///
/// Subscribes one topic per registered pattern (exact-topic match; pattern is
/// the topic). A request that sets `response_topic` gets a reply published
/// there with the request's `correlation_data` echoed back; a request without
/// one is fire-and-forget. MQTT v5 `user_properties` are surfaced as the
/// handler's `RpcContext` metadata.
///
/// # Example
///
/// ```ignore
/// app.use_rpc_adapter(toni_mqtt::MqttAdapter::new("127.0.0.1", 1883)).unwrap();
/// ```
pub struct MqttAdapter {
    host: String,
    port: u16,
    patterns: Vec<String>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl MqttAdapter {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            patterns: Vec::new(),
            callbacks: None,
        }
    }
}

#[toni::async_trait]
impl RpcAdapter for MqttAdapter {
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
        let host = self.host.clone();
        let port = self.port;
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let mut opts = MqttOptions::new(client_id("server"), host, port);
            opts.set_keep_alive(Duration::from_secs(5));
            let (client, mut eventloop) = AsyncClient::new(opts, 64);

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = client.disconnect().await;
                            break;
                        }
                    }
                    event = eventloop.poll() => {
                        match event {
                            // Subscribe on every connect, not once up front: rumqttc
                            // reconnects the socket after a drop but does not replay
                            // subscriptions, so a reconnect must re-issue them or the
                            // handler topics go silent.
                            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                                for pattern in &patterns {
                                    if let Err(e) = client.subscribe(pattern, QoS::AtLeastOnce).await {
                                        tracing::error!(error = %e, pattern, "MqttAdapter failed to subscribe");
                                    }
                                    tracing::info!(pattern, "MqttAdapter subscribing");
                                }
                            }
                            Ok(Event::Incoming(Packet::Publish(publish))) => {
                                tokio::spawn(handle_publish(publish, client.clone(), callbacks.clone()));
                            }
                            Ok(_) => {}
                            Err(e) => {
                                // rumqttc reconnects on the next poll; back off so a
                                // down broker doesn't spin the loop.
                                tracing::warn!(error = %e, "MqttAdapter connection error; retrying");
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

async fn handle_publish(
    publish: Publish,
    client: AsyncClient,
    callbacks: Arc<RpcMessageCallbacks>,
) {
    let topic = String::from_utf8_lossy(&publish.topic).to_string();
    let data = bytes_to_data(&publish.payload);

    let (response_topic, correlation_data, metadata) = match publish.properties {
        Some(p) => (
            p.response_topic,
            p.correlation_data,
            user_properties_to_metadata(&p.user_properties),
        ),
        None => (None, None, Default::default()),
    };

    let mut ctx = RpcCallInfo::new(topic);
    ctx.metadata = metadata;

    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
        .catch_unwind()
        .await;

    let Some(response_topic) = response_topic else {
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

    let props = PublishProperties {
        correlation_data,
        ..Default::default()
    };

    if let Err(e) = client
        .publish_with_properties(&response_topic, QoS::AtLeastOnce, false, response, props)
        .await
    {
        tracing::error!(error = %e, response_topic, "MqttAdapter failed to publish reply");
    }
}

fn client_id(role: &str) -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("toni-mqtt-{role}-{pid}-{nanos}")
}
