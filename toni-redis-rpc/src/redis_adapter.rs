use std::sync::Arc;

use anyhow::Result;
use futures::{FutureExt, StreamExt};
use toni::{RpcAdapter, RpcCallInfo, RpcData, RpcMessageCallbacks};

use crate::wire::RequestEnvelope;
use toni::rpc::wire::{frame_panic, frame_response};

/// Redis Pub/Sub transport adapter for the Toni RPC gateway.
///
/// Subscribes one Redis channel per registered pattern. A handler's pattern
/// maps directly to a channel name — exact match, no glob; patterns containing
/// Redis glob characters will not match anything.
///
/// **Request-response**: the caller's request envelope carries a `reply_to`
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
        let url = self.url.clone();
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
            let client = redis::Client::open(url.as_str())
                .unwrap_or_else(|e| panic!("[RedisAdapter] invalid Redis URL '{url}' — {e}"));

            // Retry the initial connect so a slow-starting Redis (container,
            // sidecar) doesn't take down the process — mirrors the NATS
            // adapter's retry-on-initial-connect posture. The publisher is a
            // ConnectionManager so reply publishes survive a reconnect on their
            // own; only the pubsub side is reconnected by hand below.
            let (publisher, mut pubsub) = connect_with_retry(&client, &url).await;

            // Streaming calls in flight, keyed by reply channel, abortable by
            // a cancel notice. Survives pubsub reconnects.
            let inflight_calls = toni::rpc::wire::Inflight::new();

            'reconnect: loop {
                for pattern in &patterns {
                    if let Err(e) = pubsub.subscribe(pattern).await {
                        tracing::error!(error = %e, pattern, "RedisAdapter failed to subscribe");
                    } else {
                        tracing::info!(pattern, "RedisAdapter subscribed");
                    }
                }
                if let Err(e) = pubsub.subscribe(crate::wire::CANCEL_CHANNEL).await {
                    tracing::error!(error = %e, "RedisAdapter failed to subscribe the cancel channel");
                }

                let mut stream = pubsub.on_message();
                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break 'reconnect;
                            }
                        }
                        msg = stream.next() => {
                            match msg {
                                Some(msg) => {
                                    let channel = msg.get_channel_name().to_string();
                                    let payload = msg.get_payload::<Vec<u8>>().unwrap_or_default();

                                    if channel == crate::wire::CANCEL_CHANNEL {
                                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                            if v.get("cancel").and_then(|c| c.as_bool()) == Some(true) {
                                                if let Some(key) = v["key"].as_str() {
                                                    inflight_calls.cancel(key);
                                                }
                                            }
                                        }
                                        continue;
                                    }

                                    let (data, reply_to, metadata) = parse_envelope(&payload);

                                    // Register a request-shaped call before
                                    // dispatch so a cancel notice can abort it
                                    // mid-handler or mid-drain. A notice racing
                                    // this registration is dropped — the cancel
                                    // channel is best-effort on a broker.
                                    let (abort_slot, guard) = match &reply_to {
                                        Some(reply_to) => {
                                            let abort_slot = Arc::new(std::sync::Mutex::new(
                                                None::<tokio::task::AbortHandle>,
                                            ));
                                            let slot = abort_slot.clone();
                                            let guard = inflight_calls.register(
                                                reply_to.clone(),
                                                move || {
                                                    if let Some(handle) =
                                                        slot.lock().unwrap().take()
                                                    {
                                                        handle.abort();
                                                    }
                                                },
                                            );
                                            (Some(abort_slot), Some(guard))
                                        }
                                        None => (None, None),
                                    };

                                    let callbacks = callbacks.clone();
                                    let publisher = publisher.clone();
                                    let handle = tokio::spawn(async move {
                                        let _guard = guard;
                                        handle_message(
                                            channel, data, reply_to, metadata, callbacks,
                                            publisher,
                                        )
                                        .await;
                                    });
                                    if let Some(slot) = abort_slot {
                                        *slot.lock().unwrap() = Some(handle.abort_handle());
                                    }
                                }
                                // The pubsub connection dropped — its stream ends.
                                // Redis pubsub has no auto-recovery, so reconnect
                                // and resubscribe by hand.
                                None => break,
                            }
                        }
                    }
                }

                drop(stream);
                tracing::warn!("RedisAdapter pubsub disconnected; reconnecting");
                match reconnect_pubsub(&client, &mut shutdown_rx).await {
                    Some(ps) => pubsub = ps,
                    None => break 'reconnect,
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

/// Open the publish connection and the pubsub connection, retrying for ~10 s
/// before giving up. The publisher is a [`ConnectionManager`], which reconnects
/// itself; the [`PubSub`] does not and is reconnected by [`reconnect_pubsub`].
///
/// [`ConnectionManager`]: redis::aio::ConnectionManager
/// [`PubSub`]: redis::aio::PubSub
async fn connect_with_retry(
    client: &redis::Client,
    url: &str,
) -> (redis::aio::ConnectionManager, redis::aio::PubSub) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match (
            client.get_connection_manager().await,
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

/// Reopen a pubsub connection after the previous one dropped, retrying with
/// backoff. Returns `None` if shutdown fires while waiting, so the serve loop
/// can exit instead of reconnecting.
async fn reconnect_pubsub(
    client: &redis::Client,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<redis::aio::PubSub> {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return None;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
        }
        match client.get_async_pubsub().await {
            Ok(pubsub) => return Some(pubsub),
            Err(e) => tracing::warn!(error = %e, "RedisAdapter pubsub reconnect failed; retrying"),
        }
    }
}

/// Decode an inbound payload. Our client always wraps the call in an
/// envelope; a payload that doesn't parse as one is a foreign publisher —
/// treated as fire-and-forget with the raw bytes as the handler payload.
fn parse_envelope(
    payload: &[u8],
) -> (
    RpcData,
    Option<String>,
    std::collections::HashMap<String, String>,
) {
    match serde_json::from_slice::<RequestEnvelope>(payload) {
        Ok(env) => (env.data, env.reply_to, env.metadata),
        Err(_) => {
            let data = match serde_json::from_slice::<serde_json::Value>(payload) {
                Ok(v) => RpcData::Json(v),
                Err(_) => RpcData::Binary(payload.to_vec()),
            };
            (data, None, Default::default())
        }
    }
}

async fn handle_message(
    channel: String,
    data: RpcData,
    reply_to: Option<String>,
    metadata: std::collections::HashMap<String, String>,
    callbacks: Arc<RpcMessageCallbacks>,
    mut publisher: redis::aio::ConnectionManager,
) {
    let mut ctx = RpcCallInfo::new(channel);
    ctx.headers = metadata;

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
        Ok(Ok(toni::RpcHandlerOutput::Stream(stream))) => {
            toni::rpc::wire::drive_reply_stream(stream, |frame| {
                let mut publisher = publisher.clone();
                let reply_to = reply_to.clone();
                async move {
                    redis::cmd("PUBLISH")
                        .arg(&reply_to)
                        .arg(frame.to_string().into_bytes())
                        .query_async::<()>(&mut publisher)
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, reply_to, "RedisAdapter stream publish error");
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

    if let Err(e) = redis::cmd("PUBLISH")
        .arg(&reply_to)
        .arg(response)
        .query_async::<()>(&mut publisher)
        .await
    {
        tracing::error!(error = %e, reply_to, "RedisAdapter failed to publish reply");
    }
}
