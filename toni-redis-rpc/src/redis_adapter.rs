use std::sync::Arc;

use anyhow::Result;
use futures::{FutureExt, StreamExt};
use toni::{RpcAdapter, RpcCallInfo, RpcData, RpcMessageCallbacks};

use crate::wire::{frame_panic, frame_response, RequestEnvelope};

/// Redis Pub/Sub transport adapter for the Toni RPC gateway.
///
/// Subscribes one Redis channel per registered pattern. A handler's pattern
/// maps directly to a channel name — exact match, no glob; patterns containing
/// Redis glob characters will not match anything.
///
/// **Request-response**: the caller's [`RequestEnvelope`] carries a `reply_to`
/// channel; the adapter publishes the framed response there.
///
/// **Fire-and-forget**: no `reply_to`; the handler runs and nothing is sent
/// back. A foreign publisher (`redis-cli PUBLISH order.shipped '{…}'`) that
/// doesn't wrap its payload in the envelope is still accepted as
/// fire-and-forget — the raw payload becomes the handler's [`RpcData`].
///
/// # Example
///
/// ```ignore
/// app.use_rpc_adapter(toni_redis_rpc::RedisAdapter::new("redis://127.0.0.1:6379")).unwrap();
/// ```
pub struct RedisAdapter {
    url: String,
    patterns: Vec<String>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl RedisAdapter {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            patterns: Vec::new(),
            callbacks: None,
        }
    }
}

#[toni::async_trait]
impl RpcAdapter for RedisAdapter {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
        self.patterns = patterns.to_vec();
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::RpcLifecycleHandle> {
        let url = self.url.clone();
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("bind() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let client = redis::Client::open(url.as_str())
                .unwrap_or_else(|e| panic!("[RedisAdapter] invalid Redis URL '{url}' — {e}"));

            // Retry the initial connect so a slow-starting Redis (container,
            // sidecar) doesn't take down the process — mirrors the NATS
            // adapter's retry-on-initial-connect posture.
            let (publisher, mut pubsub) = connect_with_retry(&client, &url).await;

            for pattern in &patterns {
                pubsub
                    .subscribe(pattern)
                    .await
                    .unwrap_or_else(|e| panic!("[RedisAdapter] failed to subscribe to '{pattern}' — {e}"));
                tracing::info!(pattern, "RedisAdapter subscribed");
            }

            let mut stream = pubsub.on_message();

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    msg = stream.next() => {
                        let Some(msg) = msg else { break };
                        let callbacks = callbacks.clone();
                        let publisher = publisher.clone();
                        let channel = msg.get_channel_name().to_string();
                        let payload = msg.get_payload::<Vec<u8>>().unwrap_or_default();

                        tokio::spawn(handle_message(channel, payload, callbacks, publisher));
                    }
                }
            }
        });

        Ok(toni::RpcLifecycleHandle::new(None, serve, move || async move {
            let _ = shutdown_tx.send(true);
            Ok(())
        }))
    }
}

/// Open the publish connection and the pubsub connection, retrying for ~10 s
/// before giving up. Both share the same backoff so a transient outage on
/// either is tolerated.
async fn connect_with_retry(
    client: &redis::Client,
    url: &str,
) -> (
    redis::aio::MultiplexedConnection,
    redis::aio::PubSub,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match (
            client.get_multiplexed_async_connection().await,
            client.get_async_pubsub().await,
        ) {
            (Ok(publisher), Ok(pubsub)) => return (publisher, pubsub),
            (publisher, pubsub) => {
                if std::time::Instant::now() >= deadline {
                    let e = publisher
                        .err()
                        .map(|e| e.to_string())
                        .or_else(|| pubsub.err().map(|e| e.to_string()))
                        .unwrap_or_default();
                    panic!("[RedisAdapter] failed to connect to '{url}' after 10s — {e}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

async fn handle_message(
    channel: String,
    payload: Vec<u8>,
    callbacks: Arc<RpcMessageCallbacks>,
    mut publisher: redis::aio::MultiplexedConnection,
) {
    // Our client always wraps the call in an envelope. A payload that doesn't
    // parse as one is a foreign publisher — treat it as fire-and-forget with
    // the raw bytes as the handler payload.
    let (data, reply_to, metadata) = match serde_json::from_slice::<RequestEnvelope>(&payload) {
        Ok(env) => (env.data, env.reply_to, env.metadata),
        Err(_) => {
            let data = match serde_json::from_slice::<serde_json::Value>(&payload) {
                Ok(v) => RpcData::Json(v),
                Err(_) => RpcData::Binary(payload),
            };
            (data, None, Default::default())
        }
    };

    let mut ctx = RpcCallInfo::new(channel);
    ctx.metadata = metadata;

    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
        .catch_unwind()
        .await;

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

    if let Err(e) = redis::cmd("PUBLISH")
        .arg(&reply_to)
        .arg(response)
        .query_async::<()>(&mut publisher)
        .await
    {
        tracing::error!(error = %e, reply_to, "RedisAdapter failed to publish reply");
    }
}
