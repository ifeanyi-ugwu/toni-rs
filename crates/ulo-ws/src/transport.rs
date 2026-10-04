use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_core::stream::BoxStream;
use http::request::Parts;
use http::{HeaderMap, Uri};
use ulo::{AppHandle, BoxError, ExecutionRef, Ext, Extensions, Inputs, LookupError, Timer, Transport};
use ulo_transport::Tracked;

use crate::connection::Connection;
use crate::envelope::{Frame, MessageId};
use crate::gateway::ConnectRefused;
use crate::session::SessionHandle;

/// The WebSocket message transport, key `"ws"`: one execution per message a gateway's
/// `#[message]` handler answers. `#[guards(ws = ..)]` scopes an entry to its handlers.
///
/// Its inputs, [`ConnectionInfo`], [`UpgradeHead`] and [`SessionHandle`], are declared by `Ws`
/// and by [`WsConnect`], each naming the other (X19), so a service reading them serves both
/// phases.
pub struct Ws;

impl Transport for Ws {
    const KEY: &'static str = "ws";

    type Cx = WsCx;
    type Reply = Reply;

    fn inputs(d: &mut Inputs) {
        d.input::<ConnectionInfo>()
            .also_seeded_by::<WsConnect>()
            .input::<UpgradeHead>()
            .also_seeded_by::<WsConnect>()
            .input::<SessionHandle>()
            .also_seeded_by::<WsConnect>();
    }
}

/// The connection phase, key `"ws_connect"`: one execution per connection, after the 101 (or
/// before it under `refuse = handshake`), running the gateway's connect guards through `dispatch`
/// and then `OnConnect`. `ulo-ws` mounts the one connect handler per gateway itself; no method
/// carries the key, so `#[guards(ws_connect = ..)]` on an impl fails X1's key assertion and
/// `connect_guards(..)` on the gateway attribute is the spelling.
pub struct WsConnect;

impl Transport for WsConnect {
    const KEY: &'static str = "ws_connect";

    type Cx = ConnectCx;
    type Reply = ConnectReply;

    fn inputs(d: &mut Inputs) {
        d.input::<ConnectionInfo>()
            .also_seeded_by::<Ws>()
            .input::<UpgradeHead>()
            .also_seeded_by::<Ws>()
            .input::<SessionHandle>()
            .also_seeded_by::<Ws>();
    }
}

/// One message's context: `Clone + Send + Sync`, every clone the same message. The execution, the
/// connection, the event and the message's `id`, and the app's `Timer`.
#[derive(Clone)]
pub struct WsCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
}

impl WsCx {
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

    /// The connection the message arrived on.
    pub fn conn(&self) -> &Connection {
        todo!()
    }

    /// The event the message named, under the gateway's event field.
    pub fn event(&self) -> &str {
        todo!()
    }

    /// The message's `id`, echoed on its answer; `None` for a fire-and-forget message.
    pub fn id(&self) -> Option<&MessageId> {
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

impl AsRef<ExecutionRef> for WsCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}

/// The connection phase's context: the execution, the connection, the upgrade request's head.
/// A connect guard reads the head and writes what it learns into the extensions, which
/// `OnConnect` and the session factory read.
#[derive(Clone)]
pub struct ConnectCx {
    pub(crate) inner: Arc<ConnectInner>,
}

pub(crate) struct ConnectInner {
    pub(crate) exec: ExecutionRef,
}

impl ConnectCx {
    pub fn exec(&self) -> &ExecutionRef {
        &self.inner.exec
    }

    pub fn extensions(&self) -> &Extensions {
        self.inner.exec.extensions()
    }

    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        self.inner.exec.resolver().ext::<T>()
    }

    pub fn conn(&self) -> &Connection {
        todo!()
    }

    /// The upgrade request's head as the client sent it.
    pub fn head(&self) -> &UpgradeHead {
        todo!()
    }

    pub fn app(&self) -> &AppHandle {
        todo!()
    }

    pub fn timer(&self) -> &Arc<dyn Timer> {
        todo!()
    }
}

impl AsRef<ExecutionRef> for ConnectCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}

/// What a message handler answers, and what an interceptor's `next.run()` returns: nothing, one
/// frame, or a stream of frames tracked for its end (transports DESIGN §4.1). A single answer is
/// written `{"id","data"}`, each streamed item likewise and the end `{"id","complete":true}`; an
/// `Err` item runs `dispatch_late` and is written `{"id","error":{..}}`, which ends the stream and
/// is reported `CutOff`.
pub enum Reply {
    None,
    One(Frame),
    Many(Tracked<BoxStream<'static, Result<Frame, BoxError>>>),
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::None => f.write_str("Reply::None"),
            Reply::One(frame) => f.debug_tuple("Reply::One").field(frame).finish(),
            Reply::Many(_) => f.write_str("Reply::Many(..)"),
        }
    }
}

/// What the connection phase answers: admitted, or refused with a close code (after the 101) or
/// a 401 or 403 (before it, under `refuse = handshake`). A guard's refusal and an error reach the
/// error handlers first; unclaimed, the kind picks the close code (transports DESIGN §4.2).
#[derive(Debug)]
pub enum ConnectReply {
    Admitted,
    Refused(ConnectRefused),
}

/// The source of the `Unimplemented` an event nothing handles raises through `recover(None, ..)`:
/// `err.downcast_ref::<CallError>()?.source_as::<NoHandler>()` tells it from a handler's own
/// `Unimplemented`, as `ulo_http::NoRoute` does for HTTP's 404.
#[derive(Debug)]
pub struct NoHandler {
    pub(crate) event: String,
}

impl NoHandler {
    /// The event the message named.
    pub fn event(&self) -> &str {
        &self.event
    }
}

impl fmt::Display for NoHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no handler for event `{}`", self.event)
    }
}

impl std::error::Error for NoHandler {}

/// The connection, as an execution input seeded into the connect phase, every message and
/// `on_disconnect`: `Dep<ConnectionInfo>` in an execution-scoped service.
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub(crate) peer: Option<SocketAddr>,
    pub(crate) path: Arc<str>,
}

impl ConnectionInfo {
    /// The peer's address; behind a proxy, the proxy's.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    /// The gateway path the connection upgraded on.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The connection's id, which `Rooms::to_client` addresses.
    pub fn id(&self) -> crate::connection::ConnId {
        todo!()
    }
}

/// The upgrade request's head as the client sent it, as an execution input: a session factory
/// reads a token from it, `session_with = |head: Dep<UpgradeHead>| ..`.
#[derive(Clone, Debug)]
pub struct UpgradeHead {
    pub(crate) parts: Arc<Parts>,
}

impl UpgradeHead {
    pub fn uri(&self) -> &Uri {
        &self.parts.uri
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }

    /// The `Sec-WebSocket-Protocol` the handshake echoed, `None` when the gateway lists none the
    /// client offered.
    pub fn subprotocol(&self) -> Option<&str> {
        todo!()
    }
}
