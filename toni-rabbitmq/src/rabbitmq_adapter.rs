use std::sync::Arc;

use anyhow::Result;
use futures::{FutureExt, StreamExt};
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicPublishOptions, QueueDeclareOptions,
};
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection};
use toni::{RpcAdapter, RpcCallInfo, RpcMessageCallbacks};

use crate::wire::{bytes_to_data, headers_to_metadata};
use toni::rpc::wire::{frame_panic, frame_response};

/// RabbitMQ (AMQP) transport adapter for the Toni RPC gateway.
///
/// Declares one queue per registered pattern, bound implicitly through the
/// default exchange (a queue is reachable by publishing to `""` with the queue
/// name as the routing key). A handler's pattern is the queue name.
///
/// **Request-response**: the delivery carries `reply_to` and `correlation_id`;
/// the adapter publishes the framed response to that queue with the same
/// correlation id.
///
/// **Fire-and-forget**: no `reply_to`; the handler runs and the delivery is
/// acked with nothing sent back.
///
/// **Metadata**: AMQP headers on the delivery are copied into the handler's
/// `RpcContext` metadata.
///
/// # Example
///
/// ```ignore
/// app.use_rpc_adapter(toni_rabbitmq::RabbitMqAdapter::new("amqp://127.0.0.1:5672/%2f")).unwrap();
/// ```
pub struct RabbitMqAdapter {
    uri: String,
    patterns: Vec<String>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl RabbitMqAdapter {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            patterns: Vec::new(),
            callbacks: None,
        }
    }
}

#[toni::async_trait]
impl RpcAdapter for RabbitMqAdapter {
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
        let uri = self.uri.clone();
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let conn = connect_with_retry(&uri).await;
            let channel = conn
                .create_channel()
                .await
                .unwrap_or_else(|e| panic!("[RabbitMqAdapter] failed to open channel — {e}"));

            // Streaming calls in flight, keyed by correlation id, abortable
            // by a cancel notice. The fanout exchange delivers every notice
            // to every instance's own queue; only the instance holding the
            // call finds a match. The queue is named (per instance) rather
            // than server-named, and lives on its own channel: lapin's
            // topology replay cannot redeclare a server-named queue, and a
            // failed replay closes the channel — which must not take the
            // pattern consumers with it.
            let inflight_calls = toni::rpc::wire::Inflight::new();
            {
                let cancel_channel = conn.create_channel().await.unwrap_or_else(|e| {
                    panic!("[RabbitMqAdapter] failed to open the cancel channel — {e}")
                });
                cancel_channel
                    .exchange_declare(
                        crate::wire::CANCEL_EXCHANGE.into(),
                        lapin::ExchangeKind::Fanout,
                        lapin::options::ExchangeDeclareOptions::default(),
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to declare the cancel exchange — {e}")
                    });
                let queue_name = format!("{}.{}", crate::wire::CANCEL_EXCHANGE, instance_id());
                let cancel_queue = cancel_channel
                    .queue_declare(
                        queue_name.as_str().into(),
                        QueueDeclareOptions {
                            auto_delete: true,
                            ..Default::default()
                        },
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to declare the cancel queue — {e}")
                    });
                cancel_channel
                    .queue_bind(
                        cancel_queue.name().clone(),
                        crate::wire::CANCEL_EXCHANGE.into(),
                        "".into(),
                        lapin::options::QueueBindOptions::default(),
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to bind the cancel queue — {e}")
                    });
                let mut cancel_consumer = cancel_channel
                    .basic_consume(
                        cancel_queue.name().clone(),
                        "".into(),
                        BasicConsumeOptions {
                            no_ack: true,
                            ..Default::default()
                        },
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to consume the cancel queue — {e}")
                    });
                let inflight_calls = inflight_calls.clone();
                tokio::spawn(async move {
                    // Holds the channel open for the consumer's lifetime.
                    let _cancel_channel = cancel_channel;
                    while let Some(delivery) = cancel_consumer.next().await {
                        let Ok(delivery) = delivery else { continue };
                        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&delivery.data)
                        else {
                            continue;
                        };
                        if v.get("cancel").and_then(|c| c.as_bool()) == Some(true) {
                            if let Some(key) = v["key"].as_str() {
                                inflight_calls.cancel(key);
                            }
                        }
                    }
                });
            }

            for pattern in &patterns {
                channel
                    .queue_declare(
                        pattern.as_str().into(),
                        QueueDeclareOptions::default(),
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to declare queue '{pattern}' — {e}")
                    });

                let consumer = channel
                    .basic_consume(
                        pattern.as_str().into(),
                        // Empty tag: the broker assigns a unique one. A fixed tag
                        // would collide across patterns on the shared channel.
                        "".into(),
                        BasicConsumeOptions::default(),
                        FieldTable::default(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[RabbitMqAdapter] failed to consume '{pattern}' — {e}")
                    });

                tracing::info!(pattern, "RabbitMqAdapter consuming");
                tokio::spawn(consume_loop(
                    consumer,
                    channel.clone(),
                    callbacks.clone(),
                    inflight_calls.clone(),
                ));
            }

            // Hold the connection open until shutdown; closing it ends every
            // consumer spawned above.
            let _ = shutdown_rx.changed().await;
            let _ = conn.close(200, "shutdown".into()).await;
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

async fn consume_loop(
    mut consumer: lapin::Consumer,
    channel: Channel,
    callbacks: Arc<RpcMessageCallbacks>,
    inflight_calls: toni::rpc::wire::Inflight,
) {
    while let Some(delivery) = consumer.next().await {
        let Ok(delivery) = delivery else { continue };

        // Register a request-shaped call before dispatch so a cancel notice
        // can abort it mid-handler or mid-drain. A notice racing this
        // registration is dropped — the cancel channel is best-effort on a
        // broker.
        let corr_key = delivery
            .properties
            .reply_to()
            .as_ref()
            .and(delivery.properties.correlation_id().as_ref())
            .map(|corr| corr.as_str().to_string());
        let (abort_slot, guard) = match corr_key {
            Some(key) => {
                let abort_slot = Arc::new(std::sync::Mutex::new(None::<tokio::task::AbortHandle>));
                let slot = abort_slot.clone();
                let guard = inflight_calls.register(key, move || {
                    if let Some(handle) = slot.lock().unwrap().take() {
                        handle.abort();
                    }
                });
                (Some(abort_slot), Some(guard))
            }
            None => (None, None),
        };

        let channel = channel.clone();
        let callbacks = callbacks.clone();
        let handle = tokio::spawn(async move {
            let _guard = guard;
            handle_delivery(delivery, channel, callbacks).await;
        });
        if let Some(slot) = abort_slot {
            *slot.lock().unwrap() = Some(handle.abort_handle());
        }
    }
}

async fn handle_delivery(
    delivery: lapin::message::Delivery,
    channel: Channel,
    callbacks: Arc<RpcMessageCallbacks>,
) {
    let reply_to = delivery.properties.reply_to().clone();
    let correlation_id = delivery.properties.correlation_id().clone();
    let metadata = delivery
        .properties
        .headers()
        .as_ref()
        .map(headers_to_metadata)
        .unwrap_or_default();

    let data = bytes_to_data(&delivery.data);
    let mut ctx = RpcCallInfo::new(delivery.routing_key.to_string());
    ctx.headers = metadata;

    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
        .catch_unwind()
        .await;

    let _ = delivery.acker.ack(BasicAckOptions::default()).await;

    let Some(reply_to) = reply_to else {
        if outcome.is_err() {
            tracing::error!("RPC handler panicked on fire-and-forget message");
        }
        return;
    };

    let response = match outcome {
        Ok(Ok(toni::RpcHandlerOutput::Stream(stream))) => {
            toni::rpc::wire::drive_reply_stream(stream, |frame| {
                let channel = channel.clone();
                let reply_to = reply_to.clone();
                let correlation_id = correlation_id.clone();
                async move {
                    let mut props = BasicProperties::default();
                    if let Some(corr) = correlation_id {
                        props = props.with_correlation_id(corr);
                    }
                    channel
                        .basic_publish(
                            "".into(),
                            reply_to.clone(),
                            BasicPublishOptions::default(),
                            frame.to_string().as_bytes(),
                            props,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| {
                            tracing::error!(
                                error = %e,
                                reply_to = reply_to.as_str(),
                                "RabbitMqAdapter stream publish error"
                            );
                        })
                }
            })
            .await;
            return;
        }
        Ok(outcome) => frame_response(outcome).into_bytes(),
        Err(_) => {
            tracing::error!("RPC handler panicked; returning error to caller");
            frame_panic().into_bytes()
        }
    };

    let mut props = BasicProperties::default();
    if let Some(corr) = correlation_id {
        props = props.with_correlation_id(corr);
    }

    if let Err(e) = channel
        .basic_publish(
            "".into(),
            reply_to.clone(),
            BasicPublishOptions::default(),
            &response,
            props,
        )
        .await
    {
        tracing::error!(error = %e, reply_to = reply_to.as_str(), "RabbitMqAdapter failed to publish reply");
    }
}

/// Names this instance's cancel queue uniquely, so two instances on one
/// broker never share one.
fn instance_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{pid}-{nanos}")
}

/// Connect with bounded retry (~10 s) so a slow-starting broker doesn't take
/// down the process — same posture as the NATS and Redis adapters. lapin's
/// default `ConnectionProperties` wires the tokio runtime itself.
///
/// `enable_auto_recover` makes lapin transparently reconnect after a dropped
/// connection and replay topology — re-declaring the queues and resuming the
/// consumers — so the consume loops keep yielding without manual re-setup.
async fn connect_with_retry(uri: &str) -> Connection {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let props = lapin::ConnectionProperties::default().enable_auto_recover();
        match Connection::connect(uri, props).await {
            Ok(conn) => return conn,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    panic!("[RabbitMqAdapter] failed to connect to '{uri}' after 10s — {e}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}
