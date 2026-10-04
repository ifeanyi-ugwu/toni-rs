use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_core::stream::BoxStream;
use ulo::{AppHandle, BoxError, ExecutionRef, Ext, Extensions, Inputs, LookupError, Timer, Transport};
use ulo_transport::{ExtractError, FromCall, Tracked};

use crate::frame::Data;

/// The RPC transport, key `"rpc"`: one execution per call or event. `#[guards(rpc = ..)]` scopes an
/// entry to its handlers. Its inputs, [`CallHeaders`] and [`LinkInfo`], are seeded into every
/// call's execution and declared by the transport itself.
pub struct Rpc;

impl Transport for Rpc {
    const KEY: &'static str = "rpc";

    type Cx = RpcCx;
    type Reply = Reply;

    fn inputs(d: &mut Inputs) {
        d.input::<CallHeaders>().input::<LinkInfo>();
    }
}

/// One call's context: `Clone + Send + Sync`, every clone the same call. The execution, the
/// pattern, the headers, the link, and the app's `Timer`.
#[derive(Clone)]
pub struct RpcCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
}

impl RpcCx {
    pub fn exec(&self) -> &ExecutionRef {
        &self.inner.exec
    }

    pub fn extensions(&self) -> &Extensions {
        self.inner.exec.extensions()
    }

    /// The extension `T` a guard wrote, as `Ext<T>` reads it.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        self.inner.exec.resolver().ext::<T>()
    }

    /// The pattern the call named.
    pub fn pattern(&self) -> &str {
        todo!()
    }

    pub fn headers(&self) -> &CallHeaders {
        todo!()
    }

    pub fn link(&self) -> &LinkInfo {
        todo!()
    }

    pub fn app(&self) -> &AppHandle {
        todo!()
    }

    /// The app's `Timer`, which anything timed inside a reply reads.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        todo!()
    }
}

impl AsRef<ExecutionRef> for RpcCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}

/// What a handler answers, and what an interceptor's `next.run()` returns: nothing (an event, or
/// a call answered with an empty `res`), one payload written as `res`, or a stream of payloads
/// tracked for its end, each written as `item` and the end as `end`. An `Err` item runs
/// `dispatch_late` and is written as `err`, which ends the stream and is reported `CutOff`.
pub enum Reply {
    None,
    One(Data),
    Many(Tracked<BoxStream<'static, Result<Data, BoxError>>>),
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::None => f.write_str("Reply::None"),
            Reply::One(data) => f.debug_tuple("Reply::One").field(data).finish(),
            Reply::Many(_) => f.write_str("Reply::Many(..)"),
        }
    }
}

/// A call's headers, the frame's `h` or the broker's own headers, as an execution input and as a
/// handler parameter. Reserved: `deadline-ms`, the remaining time, which becomes the execution's
/// deadline, and `traceparent` (W3C Trace Context).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallHeaders {
    pub(crate) entries: Vec<(String, String)>,
}

impl CallHeaders {
    pub fn new() -> Self {
        CallHeaders::default()
    }

    /// The first value under `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push((name.into(), value.into()));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.entries.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FromCall<Rpc> for CallHeaders {
    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let _ = cx;
        todo!()
    }
}

/// The link a call arrived on, as an execution input: its name, the registry's
/// `messaging.system` value on a broker, and the peer on TCP and UDP.
#[derive(Clone, Debug)]
pub struct LinkInfo {
    pub(crate) name: &'static str,
    pub(crate) peer: Option<SocketAddr>,
}

impl LinkInfo {
    /// `Link::NAME`: `tcp`, `udp`, `nats`, `redis`, `rabbitmq`, `mqtt`, `kafka`.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The caller's address on TCP and UDP; `None` on a broker.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }
}

/// The source of the `Unavailable` a pattern nothing handles raises on TCP and UDP through
/// `recover(None, ..)`, answered as `err` with `reason: "pattern_unhandled"`:
/// `err.downcast_ref::<CallError>()?.source_as::<NoHandler>()` tells it from a handler's own
/// `Unavailable`.
#[derive(Debug)]
pub struct NoHandler {
    pub(crate) pattern: String,
}

impl NoHandler {
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl fmt::Display for NoHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no handler for pattern `{}`", self.pattern)
    }
}

impl std::error::Error for NoHandler {}
