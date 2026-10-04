//! The broadcast adapter SPI, written without macros, and the in-memory adapter (transports
//! DESIGN §4.3).
//!
//! An adapter carries a frame between the processes of one application: `publish` sends it to
//! every process, and each process's `subscribe` stream delivers it to that process's own members
//! of the target. Room membership stays local to each process. Delivery is best effort and
//! ordered per sender within one process.

use std::fmt;

use bytes::Bytes;
use futures_core::stream::BoxStream;
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
    pub fn current() -> NodeId {
        todo!()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The adapter for one process, `WsModule::for_root()`'s unless `.broadcast(..)` names another.
#[derive(Clone, Default)]
pub struct InMemory {
    _private: (),
}

impl InMemory {
    pub fn new() -> Self {
        InMemory::default()
    }
}

impl BroadcastAdapter for InMemory {
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (target, frame);
        todo!()
    }

    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        let _ = node;
        todo!()
    }
}
