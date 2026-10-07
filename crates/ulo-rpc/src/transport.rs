use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use ulo::{AppHandle, BoxError, ExecutionRef, Ext, Extensions, Inputs, LookupError, Timer, Transport};
use ulo_transport::{CallError, ExtractError, FromCall, IntoReply, IntoReplyError, Tracked};

use crate::codec::Codec;
use crate::frame::Data;
use crate::link::Pattern;

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
    pub(crate) pattern: Pattern,
    pub(crate) headers: CallHeaders,
    pub(crate) link: LinkInfo,
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    /// The link's codec, which decodes the payload and encodes the reply.
    pub(crate) codec: Codec,
    /// The payload, taken once by the one parameter that consumes it.
    pub(crate) body: Mutex<Option<Body>>,
}

/// What a call carries as its payload: one `d`, or a streamed request's `in` items until
/// `in_end`, which closes the channel.
pub(crate) enum Body {
    Data(Data),
    Stream(UnboundedReceiver<Data>),
}

impl RpcCx {
    pub(crate) fn new(inner: CxInner) -> Self {
        RpcCx { inner: Arc::new(inner) }
    }

    pub(crate) fn take_body(&self) -> Option<Body> {
        self.inner.body.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
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
        self.inner.pattern.as_str()
    }

    pub fn headers(&self) -> &CallHeaders {
        &self.inner.headers
    }

    pub fn link(&self) -> &LinkInfo {
        &self.inner.link
    }

    pub fn app(&self) -> &AppHandle {
        &self.inner.app
    }

    /// The app's `Timer`, which anything timed inside a reply reads.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.inner.timer
    }

    /// The link's codec, which decodes the call's payload and encodes its reply.
    pub fn codec(&self) -> Codec {
        self.inner.codec
    }

    /// `value` encoded by the link's codec as the call's one `res`, as a handler returning a
    /// `Serialize` value is answered: what an error handler claiming an error answers with a value
    /// of its own. `Reply::None` is the empty answer.
    ///
    /// ```ignore
    /// async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
    ///     match err.downcast_ref::<CallError>().and_then(|call| call.source_as::<OutOfStock>()) {
    ///         Some(out) => Ok(cx.reply(&Backorder { sku: out.sku.clone() })?),
    ///         None => Err(err),
    ///     }
    /// }
    /// ```
    pub fn reply<T: Serialize + ?Sized>(&self, value: &T) -> Result<Reply, IntoReplyError> {
        self.codec().encode(value).map(Reply::One).map_err(IntoReplyError::new)
    }

    /// `items` as the call's reply stream, as a handler returning a stream is answered: each `Ok`
    /// encoded by the link's codec and written as `item`, the end as `end`. An `Err` item, or an
    /// item the codec refuses, runs the matched handler's error handlers on the late path and ends
    /// the stream. A link carrying no streamed reply answers the call `internal`.
    pub fn reply_stream<S, U, E>(&self, items: S) -> Reply
    where
        S: Stream<Item = Result<U, E>> + Send + 'static,
        U: Serialize + Send + 'static,
        E: Into<CallError> + Send + 'static,
    {
        self.encoded_stream(items.map(|item| item.map_err(|err| BoxError::from(err.into()))))
    }

    /// A stream of values as `Reply::Many`, each `Ok` encoded by the link's codec; a value the codec
    /// refuses becomes an `Err` item of kind `Internal` holding the `IntoReplyError`.
    pub(crate) fn encoded_stream<S, U>(&self, items: S) -> Reply
    where
        S: Stream<Item = Result<U, BoxError>> + Send + 'static,
        U: Serialize + Send + 'static,
    {
        let codec = self.codec();
        let items: BoxStream<'static, Result<Data, BoxError>> = Box::pin(items.map(move |item| {
            item.and_then(|value| codec.encode(&value).map_err(|err| BoxError::from(CallError::from(IntoReplyError::new(err)))))
        }));
        Reply::Many(Tracked::new(items, self.exec().clone()))
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
/// `dispatch_late` and is written as `err`, which ends the stream and is reported `CutOff`. An
/// error handler builds one from a value with [`RpcCx::reply`] or [`RpcCx::reply_stream`], which
/// encode with the link's codec.
pub enum Reply {
    None,
    One(Data),
    Many(Tracked<BoxStream<'static, Result<Data, BoxError>>>),
}

/// A reply already built, an interceptor's or a hand-written one, answered as it stands.
impl IntoReply<Rpc> for Reply {
    fn into_reply(self, _cx: &RpcCx) -> Result<Reply, IntoReplyError> {
        Ok(self)
    }
}

/// A payload already encoded by the link's codec, answered as one `res`.
impl IntoReply<Rpc> for Data {
    fn into_reply(self, _cx: &RpcCx) -> Result<Reply, IntoReplyError> {
        Ok(Reply::One(self))
    }
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
        Ok(cx.inner.headers.clone())
    }
}

/// The link a call arrived on, as an execution input: its name, the registry's
/// `messaging.system` value on a broker, and the peer on TCP and UDP, which those links attach to
/// every delivery's reply path, an event's included.
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
    pub(crate) fn new(pattern: &str) -> Self {
        NoHandler { pattern: pattern.to_owned() }
    }

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
