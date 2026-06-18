use std::sync::Arc;

use anyhow::Result;
use futures::{FutureExt, StreamExt};
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicPublishOptions, QueueDeclareOptions,
};
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection};
use toni::{RpcAdapter, RpcCallInfo, RpcMessageCallbacks};

use crate::wire::{bytes_to_data, frame_panic, frame_response, headers_to_metadata};

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
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
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
            .expect("bind() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let conn = connect_with_retry(&uri).await;
            let channel = conn
                .create_channel()
                .await
                .unwrap_or_else(|e| panic!("[RabbitMqAdapter] failed to open channel — {e}"));

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
                tokio::spawn(consume_loop(consumer, channel.clone(), callbacks.clone()));
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
) {
    while let Some(delivery) = consumer.next().await {
        let Ok(delivery) = delivery else { continue };
        tokio::spawn(handle_delivery(
            delivery,
            channel.clone(),
            callbacks.clone(),
        ));
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
    ctx.metadata = metadata;

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
        Ok(outcome) => frame_response(outcome),
        Err(_) => {
            tracing::error!("RPC handler panicked; returning error to caller");
            frame_panic()
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
