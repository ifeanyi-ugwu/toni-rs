//! The broadcast adapter SPI, written without macros, and the in-memory adapter (transports
//! DESIGN §4.3).
//!
//! An adapter carries a frame between the processes of one application: `publish` sends it to
//! every process, and each process's `subscribe` stream delivers it to that process's own members
//! of the target. Room membership stays local to each process. Delivery is best effort and
//! ordered per sender within one process.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_core::stream::BoxStream;
use async_broadcast::{InactiveReceiver, RecvError, Sender};
use serde::{Deserialize, Serialize};
use ulo::{BoxError, BoxFuture};

use crate::connection::ConnId;

/// The broadcast SPI: an in-memory adapter for one process, `ulo_ws_redis::Redis` across
/// processes. Set on the module, `WsModule::for_root().broadcast(adapter)`; the module binds it
/// as `dyn BroadcastAdapter`.
pub trait BroadcastAdapter: Send + Sync + 'static {
    /// Sends `frame`, an encoded envelope, to every process's members of `target`.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>>;

    /// What every process published, this one's included, for the process `node` to deliver to
    /// its own members.
    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)>;
}

/// Who a broadcast reaches.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The gateway path or namespace addressed; `None` addresses every gateway.
    pub gateway: Option<String>,
    pub audience: Audience,
    /// Connections left out.
    pub except: Vec<ConnId>,
}

impl Target {
    pub fn new(gateway: Option<String>, audience: Audience, except: Vec<ConnId>) -> Self {
        Target { gateway, audience, except }
    }
}

/// The members of a broadcast within its gateway or gateways.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Audience {
    All,
    Room(String),
    Client(ConnId),
}

/// One process among those sharing an adapter: random per process, part of every `ConnId` it
/// hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub(crate) u128);

impl NodeId {
    /// This process's id, the same for its whole life.
    ///
    /// Drawn from the standard library's per-process random hash keys, the process id and the
    /// clock, so two processes started together on one host still differ.
    pub fn current() -> NodeId {
        static CURRENT: OnceLock<NodeId> = OnceLock::new();
        *CURRENT.get_or_init(|| {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
            let half = |salt: u64| {
                let mut hasher = RandomState::new().build_hasher();
                hasher.write_u64(salt);
                hasher.write_u32(std::process::id());
                hasher.write_u128(nanos);
                hasher.finish()
            };
            NodeId((u128::from(half(0)) << 64) | u128::from(half(1)))
        })
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The adapter for one process, `WsModule::for_root()`'s unless `.broadcast(..)` names another.
///
/// A clone shares the channel. A subscriber more than 1024 broadcasts behind skips the ones it
/// missed, logged at `warn`: delivery is best effort.
#[derive(Clone, Default)]
pub struct InMemory {
    inner: Arc<Channel>,
}

/// `async-broadcast` in overflow mode: the sender never waits, and a full queue drops its oldest
/// broadcast, which a subscriber that had not read it learns as `Overflowed`.
struct Channel {
    sender: Sender<(Target, Bytes)>,
    /// Keeps the channel open while no subscriber is active; it holds no broadcast.
    _open: InactiveReceiver<(Target, Bytes)>,
}

impl Default for Channel {
    fn default() -> Self {
        let (mut sender, receiver) = async_broadcast::broadcast(1024);
        sender.set_overflow(true);
        Channel { sender, _open: receiver.deactivate() }
    }
}

impl InMemory {
    pub fn new() -> Self {
        InMemory::default()
    }
}

impl BroadcastAdapter for InMemory {
    /// With no subscriber there is no member to deliver to, which is not a failure.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        // `Inactive` when no subscriber is active. In overflow mode a full queue drops its oldest
        // broadcast rather than refusing this one.
        let _ = self.inner.sender.try_broadcast((target, frame));
        Box::pin(async { Ok(()) })
    }

    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        let _ = node;
        // Starts at the next broadcast, as a subscriber joining late should.
        let receiver = self.inner.sender.new_receiver();
        Box::pin(futures_util::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv_direct().await {
                    Ok(item) => return Some((item, receiver)),
                    Err(RecvError::Overflowed(missed)) => {
                        tracing::warn!(missed, "a WebSocket broadcast subscriber fell behind and skipped broadcasts");
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        }))
    }
}
