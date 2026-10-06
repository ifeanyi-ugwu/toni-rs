//! The Redis broadcast adapter for `ulo-ws` (transports DESIGN §4.3): one Pub/Sub channel per room
//! or client, each process delivering to its own members. Membership stays local to each process
//! by construction, delivery is best effort and ordered per sender within one process, and the
//! API is the in-memory adapter's.
//!
//! ```ignore
//! #[module(imports = [ulo_ws::WsModule::for_root().broadcast(ulo_ws_redis::Redis::url("redis://cache:6379"))])]
//! pub struct AppModule;
//! ```
//!
//! The crate ships the adapter value and no module: a second module binding
//! `dyn BroadcastAdapter` beside `WsModule`'s would be `DuplicateBinding` at `wire()`.
//!
//! The channels: `ulo:ws:room:<room>` for a room, `ulo:ws:node:<node>` for a client, addressed to
//! the process its id names, and `ulo:ws:all` for everyone. Each message is the target as JSON, a
//! newline, then the encoded broadcast. A process subscribes to its node's channel and
//! `ulo:ws:all`, and pattern-subscribes to `ulo:ws:room:*`, filtering rooms against its own
//! members on delivery, since the adapter learns of no membership change.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use redis::aio::MultiplexedConnection;
use tokio::sync::{Mutex, mpsc};
use ulo::{BoxError, BoxFuture};
use ulo_ws::{Audience, BroadcastAdapter, NodeId, Target};

/// How long a lost subscription waits before connecting again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// The Redis adapter. The URL is parsed and the connection made lazily, at the first publish or
/// subscribe; `rediss://` selects TLS.
#[derive(Clone)]
pub struct Redis {
    pub(crate) url: String,
    publisher: Arc<Mutex<Option<MultiplexedConnection>>>,
}

impl Redis {
    /// The adapter on `url`, `redis://host:6379` or `rediss://host:6380`.
    pub fn url(url: impl Into<String>) -> Self {
        Redis { url: url.into(), publisher: Arc::new(Mutex::new(None)) }
    }
}

fn channel(target: &Target) -> String {
    match &target.audience {
        Audience::Room(room) => format!("ulo:ws:room:{room}"),
        Audience::Client(id) => format!("ulo:ws:node:{}", id.node()),
        _ => "ulo:ws:all".to_owned(),
    }
}

fn encode(target: &Target, frame: &[u8]) -> Result<Vec<u8>, serde_json::Error> {
    let mut message = serde_json::to_vec(target)?;
    message.push(b'\n');
    message.extend_from_slice(frame);
    Ok(message)
}

fn decode(message: &[u8]) -> Option<(Target, Bytes)> {
    let split = message.iter().position(|&byte| byte == b'\n')?;
    let target = serde_json::from_slice(&message[..split]).ok()?;
    Some((target, Bytes::copy_from_slice(&message[split + 1..])))
}

impl BroadcastAdapter for Redis {
    /// A failed publish drops the cached connection, so the next publish connects again.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        let url = self.url.clone();
        let publisher = Arc::clone(&self.publisher);
        Box::pin(async move {
            let message = encode(&target, &frame)?;
            let mut cached = publisher.lock().await;
            let mut connection = match cached.take() {
                Some(connection) => connection,
                None => redis::Client::open(url.as_str())?.get_multiplexed_async_connection().await?,
            };
            redis::cmd("PUBLISH").arg(channel(&target)).arg(message.as_slice()).query_async::<i64>(&mut connection).await?;
            *cached = Some(connection);
            Ok(())
        })
    }

    /// Runs the subscription on its own task, reconnecting after a second when Redis goes away;
    /// broadcasts published while it is away are lost, as Pub/Sub delivers to subscribers present.
    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        let (sender, receiver) = mpsc::unbounded_channel::<(Target, Bytes)>();
        let url = self.url.clone();
        tokio::spawn(async move {
            while !sender.is_closed() {
                if let Err(error) = listen(&url, node, &sender).await {
                    tracing::warn!(%error, "the Redis broadcast subscription failed; reconnecting");
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        });
        Box::pin(futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|item| (item, receiver))
        }))
    }
}

/// One subscription, until Redis goes away or the stream's reader is dropped.
async fn listen(url: &str, node: NodeId, sender: &mpsc::UnboundedSender<(Target, Bytes)>) -> Result<(), BoxError> {
    let mut pubsub = redis::Client::open(url)?.get_async_pubsub().await?;
    pubsub.subscribe("ulo:ws:all").await?;
    pubsub.subscribe(format!("ulo:ws:node:{node}")).await?;
    pubsub.psubscribe("ulo:ws:room:*").await?;
    let mut messages = pubsub.into_on_message();
    while let Some(message) = messages.next().await {
        match decode(message.get_payload_bytes()) {
            Some(item) => {
                if sender.send(item).is_err() {
                    return Ok(());
                }
            }
            None => tracing::warn!(channel = message.get_channel_name(), "a Redis broadcast did not decode; it is dropped"),
        }
    }
    Err(BoxError::from("the Redis subscription ended"))
}
