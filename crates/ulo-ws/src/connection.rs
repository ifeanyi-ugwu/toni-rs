//! One connection: the read loop, the outbound queue, the limits and close codes, keep-alive on the
//! app's `Timer`, the per-message execution, and `on_disconnect` as a terminal execution
//! (transports DESIGN §4.1, §4.2).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ulo::{Closed, Execution, Timer};

use crate::broadcast::NodeId;
use crate::envelope::Frame;
use crate::rooms::BroadcastError;
use crate::session::SessionHandle;
use crate::transport::{ConnectionInfo, UpgradeHead};

/// A connection's id, unique across every process sharing a broadcast adapter: the process's
/// [`NodeId`] and a counter. `Rooms::to_client(id)` addresses it, and `except([id])` leaves it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnId {
    pub(crate) node: NodeId,
    pub(crate) seq: u64,
}

impl ConnId {
    /// The process the connection lives on.
    pub fn node(&self) -> NodeId {
        self.node
    }
}

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.node, self.seq)
    }
}

/// A handle to one open connection: `Clone + Send + Sync`, every clone the same connection.
/// `OnConnect` joins rooms through it, `OnDisconnect` receives it, and a hand-written
/// [`Gateway`](crate::Gateway) sends frames and opens executions through it.
#[derive(Clone)]
pub struct Connection {
    pub(crate) inner: Arc<ConnInner>,
}

pub(crate) struct ConnInner {
    pub(crate) id: ConnId,
}

impl Connection {
    pub fn id(&self) -> ConnId {
        self.inner.id
    }

    /// Joins `room` on this connection's gateway. Membership is local to this process.
    pub async fn join(&self, room: impl Into<String>) {
        let _ = room;
        todo!()
    }

    pub async fn leave(&self, room: &str) {
        let _ = room;
        todo!()
    }

    /// The rooms this connection is in.
    pub fn rooms(&self) -> Vec<String> {
        todo!()
    }

    /// Queues `frame` as one data message, as it stands: no envelope is added. Over
    /// `max_outbound` the connection closes with 1008 "slow consumer", or the oldest queued
    /// message is dropped under `overflow = drop_oldest`; `Err` once the connection has closed.
    pub async fn send(&self, frame: Frame) -> Result<(), BroadcastError> {
        let _ = frame;
        todo!()
    }

    /// Closes the connection with `code` and `reason`, which `on_disconnect` receives as
    /// `DisconnectReason::ServerClose`. `code` and `reason` are checked as `ConnectRefused::code`
    /// checks them; an invalid pair closes with 1011.
    pub async fn close(&self, code: u16, reason: &str) {
        let _ = (code, reason);
        todo!()
    }

    /// An execution in the gateway's module with this connection's inputs seeded:
    /// [`ConnectionInfo`], [`UpgradeHead`] and [`SessionHandle`]. A hand-written gateway runs one
    /// unit of work in it, a graphql-transport-ws `subscribe` for example, and cancels it with
    /// `cancel_with(CancelReason::ClientCancelled)`. Refused once the app's drain has begun.
    pub fn open_execution(&self) -> Result<Execution, Closed> {
        todo!()
    }

    pub fn info(&self) -> &ConnectionInfo {
        todo!()
    }

    pub fn head(&self) -> &UpgradeHead {
        todo!()
    }

    pub fn session(&self) -> &SessionHandle {
        todo!()
    }

    /// The app's `Timer`, which a hand-written gateway's own clocks read.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        todo!()
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").field("id", &self.inner.id).finish()
    }
}
