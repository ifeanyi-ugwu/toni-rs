//! The RPC client (transports DESIGN §5.4): requests, events and the streamed shapes over a link's
//! client side, connected lazily on the first call.
//!
//! ```ignore
//! self.billing.request::<_, Invoice>("invoices.create", &NewInvoice::from(order))
//!     .header("tenant", order.tenant())
//!     .timeout(Duration::from_secs(2))
//!     .await
//! ```
//!
//! One task per connection, spawned on the client's runtime, routes the link's reply lane to the
//! calls waiting on it, by `id`. A reply for an id no call waits on any more, a second reply on a
//! `FanOut` link or one arriving after its call gave up, is dropped. A lost reply lane fails every
//! call waiting on it `Unavailable`, and the next call connects again; a `goaway` lets the calls in
//! flight finish and sends the next call over a new connection.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::{FutureExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::{Bound, BoxError, BoxFuture, CancelReason, ExecutionRef, Runtime, Timer};
use ulo_transport::{Classify, Detail, Details, ErrorKind};

use crate::codec::Codec;
use crate::dispatch::reason;
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{FrameTooLarge, FrameUnencodable, Link, NoDestination, Outbound, Pattern, ReplyTo};
use crate::transport::CallHeaders;

/// A client's timeout at `Bound::Default`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

fn timeout_of(timeout: Bound) -> Option<Duration> {
    match timeout {
        Bound::Default => Some(DEFAULT_TIMEOUT),
        Bound::After(after) => Some(after),
        Bound::Unbounded => None,
    }
}

/// A zero timeout given to an RPC client, which would time out every call:
/// `Bound::After(Duration::ZERO)` refused by [`RpcClient::timeout`] where it is written, and by
/// `RpcClientModule::timeout` when the app wires. `Bound::Unbounded` is the spelling of no
/// timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroTimeout {
    setter: &'static str,
    link: &'static str,
}

impl ZeroTimeout {
    pub(crate) fn of_module(link: &'static str) -> Self {
        ZeroTimeout { setter: "RpcClientModule::timeout", link }
    }

    /// The link's name, `Link::NAME`.
    pub fn link(&self) -> &'static str {
        self.link
    }
}

impl fmt::Display for ZeroTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}(Bound::After(Duration::ZERO))` on the {} link would time out every call; write `Bound::Unbounded` to turn the \
             timeout off",
            self.setter, self.link
        )
    }
}

impl Error for ZeroTimeout {}

/// A client of one link, bound by `RpcClientModule` as `Dep<RpcClient, Billing>` under the
/// module's qualifier, or built outside an app with [`RpcClient::new`]. Cheap to clone. Its tasks
/// and every timeout run on the runtime it was given, the app's inside an app; dropping a pending
/// request or a stream sends `cancel`, spawned on that runtime. There is no ambient execution: a
/// call made inside one forwards its deadline and cancellation with [`Call::within`].
#[derive(Clone)]
pub struct RpcClient {
    pub(crate) inner: Arc<ClientInner>,
    /// Every call's timeout unless the call sets its own; `None` for `Bound::Unbounded`. Held by
    /// the handle, so [`timeout`](Self::timeout) on one clone leaves the others' as they were.
    timeout: Option<Duration>,
}

type Connect = Box<dyn Fn() -> BoxFuture<'static, Result<Outbound, BoxError>> + Send + Sync>;

type Close = Box<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

pub(crate) struct ClientInner {
    link: &'static str,
    connect: Connect,
    /// The link's `close`, for a client that owns its link, one built by [`RpcClient::new`]; a
    /// module's client leaves it to the module's destroy hook.
    close: Option<Close>,
    codec: Codec,
    runtime: Arc<dyn Runtime>,
    /// Held across `connect`, so concurrent first calls share one connection.
    conn: async_lock::Mutex<Option<Arc<Conn>>>,
    next_id: AtomicU64,
}

/// One connection of the link's client side.
struct Conn {
    send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    pending: Mutex<HashMap<u64, Waiting>>,
    /// False once the reply lane ended or the server sent `goaway`: no new call goes over it.
    open: AtomicBool,
    /// The client's runtime, which a dropped call's `cancel` is spawned on.
    runtime: Arc<dyn Runtime>,
}

/// Where the reply frames of one call go, and a streamed request's pump reports its failure.
type Waiting = UnboundedSender<Result<Frame, RpcError>>;

impl Conn {
    fn pending(&self) -> MutexGuard<'_, HashMap<u64, Waiting>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, pattern: &Pattern, frame: Frame, reply_to: Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> {
        (self.send)(pattern.clone(), frame, reply_to)
    }

    /// Registers call `id` for its replies: `false`, registering nothing, once the reply lane has
    /// ended or a `goaway` arrived. Checked under the lock [`ended`](Self::ended) clears under, so
    /// a call registered here is answered by the lane or failed by its end, never left waiting on
    /// a lane that has already gone.
    fn register(&self, id: u64, waiting: Waiting) -> bool {
        let mut pending = self.pending();
        if !self.open.load(Ordering::Acquire) {
            return false;
        }
        pending.insert(id, waiting);
        true
    }

    /// The reply lane's end: no new call goes over the connection, and every call waiting on it
    /// fails `Unavailable`.
    fn ended(&self) {
        let mut pending = self.pending();
        self.open.store(false, Ordering::Release);
        pending.clear();
    }
}

impl RpcClient {
    /// A client of `link` outside an app, its tasks spawned on `runtime`, which also times its
    /// calls: five seconds each unless [`timeout`](Self::timeout) or the call sets another.
    ///
    /// ```ignore
    /// let billing = RpcClient::new(ulo_rpc_tcp::Tcp::new("billing:7000"), Arc::new(ulo_tokio::Tokio::current()))
    ///     .timeout(Bound::After(Duration::from_secs(2)))?;
    /// ```
    ///
    /// The client owns the link. Once its last clone and the last call it made are dropped, it
    /// releases its connection and spawns the link's `close` on `runtime`. A runtime that drops
    /// that task before it finishes, as one that has shut down does, leaves the link's close
    /// undone, logged at `warn`.
    pub fn new<L: Link>(link: L, runtime: Arc<dyn Runtime>) -> Self {
        let link = Arc::new(link);
        let closing = Arc::clone(&link);
        let close: Close = Box::new(move || {
            let link = Arc::clone(&closing);
            Box::pin(async move { link.close().await })
        });
        RpcClient::build(link, Bound::Default, runtime, Some(close))
    }

    /// `RpcClientModule`'s client, whose link the module's destroy hook closes.
    pub(crate) fn of_module<L: Link>(link: Arc<L>, timeout: Bound, runtime: Arc<dyn Runtime>) -> Self {
        RpcClient::build(link, timeout, runtime, None)
    }

    fn build<L: Link>(link: Arc<L>, timeout: Bound, runtime: Arc<dyn Runtime>, close: Option<Close>) -> Self {
        let codec = Codec::of(&link.capabilities());
        let connect: Connect = Box::new(move || {
            let link = Arc::clone(&link);
            Box::pin(async move { link.connect().await })
        });
        RpcClient {
            inner: Arc::new(ClientInner {
                link: L::NAME,
                connect,
                close,
                codec,
                runtime,
                conn: async_lock::Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
            timeout: timeout_of(timeout),
        }
    }

    /// Every call's timeout unless the call sets its own, as `RpcClientModule::timeout` sets it
    /// for a module's client: five seconds at `Bound::Default`, `Bound::Unbounded` for none. This
    /// handle and the clones made from it after carry it; clones made before keep theirs, sharing
    /// the connection all the same.
    ///
    /// `Bound::After(Duration::ZERO)`, which would time out every call, is refused as
    /// [`ZeroTimeout`], and the handle with it, so `RpcClient::new(link, runtime).timeout(b)?`
    /// fails on the line that wrote the zero. `RpcClientModule` refuses it when the app wires.
    pub fn timeout(mut self, timeout: Bound) -> Result<Self, ZeroTimeout> {
        if timeout == Bound::After(Duration::ZERO) {
            return Err(ZeroTimeout { setter: "RpcClient::timeout", link: self.inner.link });
        }
        self.timeout = timeout_of(timeout);
        Ok(self)
    }

    /// A request answered by one reply.
    pub fn request<Req, Res>(&self, pattern: &str, req: &Req) -> Call<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let payload = encode(self.inner.codec, req);
        Call { _res: PhantomData, spec: Spec::new(self, pattern, Request::One(payload)) }
    }

    /// An event, answered by nothing.
    pub fn emit<T: Serialize + ?Sized>(&self, pattern: &str, data: &T) -> Emit {
        let payload = encode(self.inner.codec, data);
        Emit { spec: Spec::new(self, pattern, Request::One(payload)) }
    }

    /// A request answered by a stream of replies.
    pub fn stream<Req, Res>(&self, pattern: &str, req: &Req) -> StreamCall<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let payload = encode(self.inner.codec, req);
        StreamCall { _res: PhantomData, spec: Spec::new(self, pattern, Request::One(payload)) }
    }

    /// A streamed request answered by one reply.
    pub fn send_stream<S, Res>(&self, pattern: &str, items: S) -> Call<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        Call { _res: PhantomData, spec: Spec::new(self, pattern, self.items(items)) }
    }

    /// A streamed request answered by a stream of replies.
    pub fn duplex<S, Res>(&self, pattern: &str, items: S) -> StreamCall<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        StreamCall { _res: PhantomData, spec: Spec::new(self, pattern, self.items(items)) }
    }

    fn items<S>(&self, items: S) -> Request
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
    {
        let codec = self.inner.codec;
        Request::Stream(Box::pin(items.map(move |item| encode(codec, &item))))
    }
}

/// `value` as a payload. A JSON link carries raw bytes only as an array of numbers, so a payload
/// that writes them is refused here, before any I/O.
fn encode<T: Serialize + ?Sized>(codec: Codec, value: &T) -> Result<Data, RpcError> {
    match codec.encode_checked(value) {
        Ok((_, true)) if !codec.binary() => Err(RpcError::new(
            ErrorKind::BadRequest,
            "a binary payload cannot travel on a JSON link; set `.codec(Codec::Cbor)` on the link",
        )
        .with_reason("binary_unsupported")),
        Ok((data, _)) => Ok(data),
        Err(err) => Err(RpcError::new(ErrorKind::BadRequest, format!("the payload could not be encoded: {err}"))),
    }
}

/// What every builder carries.
pub(crate) struct Spec {
    client: RpcClient,
    pattern: Pattern,
    request: Request,
    headers: CallHeaders,
    /// The call's own timeout, in place of the client's.
    timeout: Option<Duration>,
    within: Option<ExecutionRef>,
}

enum Request {
    One(Result<Data, RpcError>),
    Stream(BoxStream<'static, Result<Data, RpcError>>),
}

impl Spec {
    fn new(client: &RpcClient, pattern: &str, request: Request) -> Self {
        Spec { client: client.clone(), pattern: Pattern::from(pattern), request, headers: CallHeaders::new(), timeout: None, within: None }
    }

    /// The wait the call is bounded by: its own timeout or the client's, and the calling
    /// execution's remaining time when that is shorter. Stamps `deadline-ms` with that remaining
    /// time; refuses a call whose execution's deadline has passed, before any I/O.
    fn limit(&mut self) -> Result<Option<Duration>, RpcError> {
        let inner = &self.client.inner;
        let own = self.timeout.or(self.client.timeout);
        let Some(deadline) = self.within.as_ref().and_then(ExecutionRef::deadline) else {
            return Ok(own);
        };
        let remaining = deadline.saturating_duration_since(inner.runtime.now());
        if remaining.is_zero() {
            return Err(RpcError::new(ErrorKind::Timeout, "the calling execution's deadline has passed"));
        }
        self.headers.entries.retain(|(name, _)| name != "deadline-ms");
        let millis = remaining.as_millis().max(1);
        self.headers.insert("deadline-ms", millis.to_string());
        Ok(Some(own.map_or(remaining, |own| own.min(remaining))))
    }

    /// Connects, registers the call, and sends its opening frame; a streamed request's items follow
    /// from a task of their own.
    async fn open(self) -> Result<Exchange, RpcError> {
        let Spec { client, pattern, request, headers, within, .. } = self;
        let inner = &client.inner;
        let request = match request {
            Request::One(payload) => Request::One(Ok(payload?)),
            stream => stream,
        };
        let (conn, id, waiting, replies) = inner.register().await?;
        let mut pending = Pending { conn: Arc::clone(&conn), id, pattern: pattern.clone(), settled: false };
        let text = pattern.as_str().to_owned();
        let sent = match request {
            Request::One(payload) => {
                let data = payload?;
                conn.send(&pattern, Frame::Req { id, pattern: text, headers, data }, Some(ReplyTo { id })).await
            }
            Request::Stream(items) => {
                let opened = conn.send(&pattern, Frame::Open { id, pattern: text, headers }, Some(ReplyTo { id })).await;
                if opened.is_ok() {
                    // Detached: the pump stops once the call has ended.
                    drop(inner.runtime.spawn(Box::pin(pump(Arc::clone(&conn), pattern, id, items, waiting))));
                }
                opened
            }
        };
        if let Err(err) = sent {
            // Nothing reached a server that a `cancel` would stop.
            pending.settled = true;
            return Err(send_error(err));
        }
        let timer: Arc<dyn Timer> = inner.runtime.clone();
        Ok(Exchange { replies, pending, codec: inner.codec, timer, within, _client: client.clone() })
    }
}

impl ClientInner {
    /// A connection with a new call registered on it, and where its replies arrive. A connection
    /// whose reply lane ends between [`connection`](Self::connection) handing it over and the
    /// registration is replaced once by a new one; a second that ends as fast fails the call
    /// `Unavailable`.
    async fn register(&self) -> Result<(Arc<Conn>, u64, Waiting, UnboundedReceiver<Result<Frame, RpcError>>), RpcError> {
        for _ in 0..2 {
            let conn = self.connection().await?;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (waiting, replies) = mpsc::unbounded();
            if conn.register(id, waiting.clone()) {
                return Ok((conn, id, waiting, replies));
            }
        }
        Err(lost())
    }

    async fn connection(&self) -> Result<Arc<Conn>, RpcError> {
        let mut slot = self.conn.lock().await;
        if let Some(conn) = slot.as_ref().filter(|conn| conn.open.load(Ordering::Acquire)) {
            return Ok(Arc::clone(conn));
        }
        let Outbound { send, replies } = (self.connect)().await.map_err(|err| {
            RpcError::new(ErrorKind::Unavailable, format!("the {} link could not connect: {err}", self.link))
        })?;
        let conn = Arc::new(Conn {
            send,
            pending: Mutex::new(HashMap::new()),
            open: AtomicBool::new(true),
            runtime: Arc::clone(&self.runtime),
        });
        // Detached: it ends with the reply lane, or at the next frame once the connection is gone.
        drop(self.runtime.spawn(Box::pin(route_replies(Arc::downgrade(&conn), replies))));
        *slot = Some(Arc::clone(&conn));
        Ok(conn)
    }
}

/// The reply lane of one connection, routed by id. It holds the connection weakly, so a client
/// dropped with no call in flight releases it at the next frame.
async fn route_replies(conn: Weak<Conn>, mut replies: BoxStream<'static, Frame>) {
    while let Some(frame) = replies.next().await {
        let Some(conn) = conn.upgrade() else { return };
        if matches!(frame, Frame::Goaway) {
            conn.open.store(false, Ordering::Release);
            continue;
        }
        let Some(id) = frame.id() else { continue };
        let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
        let waiting = {
            let mut pending = conn.pending();
            if ends { pending.remove(&id) } else { pending.get(&id).cloned() }
        };
        if let Some(waiting) = waiting {
            let _ = waiting.unbounded_send(Ok(frame));
        }
    }
    if let Some(conn) = conn.upgrade() {
        conn.ended();
    }
}

/// A streamed request's items as `in` frames, then `in_end`. A failure goes to the waiting call,
/// which fails with it; the pump stops once the call has ended.
async fn pump(conn: Arc<Conn>, pattern: Pattern, id: u64, mut items: BoxStream<'static, Result<Data, RpcError>>, waiting: Waiting) {
    while let Some(item) = items.next().await {
        if waiting.is_closed() {
            return;
        }
        let data = match item {
            Ok(data) => data,
            Err(err) => {
                let _ = waiting.unbounded_send(Err(err));
                return;
            }
        };
        if let Err(err) = conn.send(&pattern, Frame::In { id, data }, None).await {
            let _ = waiting.unbounded_send(Err(send_error(err)));
            return;
        }
    }
    if let Err(err) = conn.send(&pattern, Frame::InEnd { id }, None).await {
        let _ = waiting.unbounded_send(Err(send_error(err)));
    }
}

/// A call registered on its connection. Dropped before its reply settled it, a timeout, a cancel
/// or the caller's own drop, it sends `cancel`.
struct Pending {
    conn: Arc<Conn>,
    id: u64,
    pattern: Pattern,
    settled: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.conn.pending().remove(&self.id);
        if self.settled {
            return;
        }
        let cancel = self.conn.send(&self.pattern, Frame::Cancel { id: self.id }, None);
        drop(self.conn.runtime.spawn(Box::pin(async move {
            let _ = cancel.await;
        })));
    }
}

/// A client that owns its link closes it from a task on its runtime, a drop being unable to
/// await. Its connection is released as its fields drop, whatever the runtime does with the task.
impl Drop for ClientInner {
    fn drop(&mut self) {
        let Some(close) = &self.close else { return };
        let closing = close();
        let link = self.link;
        let unfinished = Unfinished { link, finished: false };
        drop(self.runtime.spawn(Box::pin(async move {
            let closed = closing.await;
            unfinished.finish();
            if let Err(error) = closed {
                tracing::warn!(%error, link, "the RPC client's link did not close cleanly");
            }
        })));
    }
}

/// The link's close spawned by a dropped client, reported at `warn` if its task is dropped
/// before the close finished: a runtime that has shut down drops a task it is handed unrun.
struct Unfinished {
    link: &'static str,
    finished: bool,
}

impl Unfinished {
    fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                link = self.link,
                "the RPC client was dropped and its runtime dropped the link's close before it finished; its connection was released"
            );
        }
    }
}

/// One call's reply frames.
struct Exchange {
    replies: UnboundedReceiver<Result<Frame, RpcError>>,
    pending: Pending,
    codec: Codec,
    timer: Arc<dyn Timer>,
    within: Option<ExecutionRef>,
    /// Keeps a client that owns its link from closing it while this call is under way.
    _client: RpcClient,
}

impl Exchange {
    async fn single<Res: DeserializeOwned>(mut self) -> Result<Res, RpcError> {
        match self.replies.next().await {
            Some(Ok(Frame::Res { data, .. })) => {
                self.pending.settled = true;
                decode(self.codec, &data)
            }
            Some(Ok(Frame::Err { error, .. })) => {
                self.pending.settled = true;
                Err(RpcError::from_body(error))
            }
            Some(Ok(other)) => Err(unexpected(&other)),
            Some(Err(err)) => Err(err),
            None => Err(lost()),
        }
    }
}

fn decode<Res: DeserializeOwned>(codec: Codec, data: &Data) -> Result<Res, RpcError> {
    codec.decode(data).map_err(|err| RpcError::new(ErrorKind::Internal, format!("the reply could not be decoded: {err}")))
}

fn unexpected(frame: &Frame) -> RpcError {
    RpcError::new(ErrorKind::Internal, format!("the server answered with an unexpected `{}` frame", frame.kind()))
}

fn lost() -> RpcError {
    RpcError::new(ErrorKind::Unavailable, "the link closed before the reply arrived")
}

/// A link's refusal of a frame, mapped one way on every link.
fn send_error(err: BoxError) -> RpcError {
    if err.is::<NoDestination>() {
        RpcError::new(ErrorKind::Unavailable, err.to_string()).with_reason("no_destination")
    } else if err.is::<FrameTooLarge>() {
        RpcError::new(ErrorKind::BadRequest, err.to_string()).with_reason("payload_too_large")
    } else if err.is::<FrameUnencodable>() {
        RpcError::new(ErrorKind::Internal, err.to_string())
    } else {
        RpcError::new(ErrorKind::Unavailable, format!("the link refused the frame: {err}"))
    }
}

/// `work` bounded by `limit` on the client's runtime and by the calling execution's cancellation.
async fn bounded<T>(
    timer: &dyn Timer,
    limit: Option<Duration>,
    within: Option<&ExecutionRef>,
    work: impl Future<Output = Result<T, RpcError>>,
) -> Result<T, RpcError> {
    let expired = async {
        match limit {
            Some(after) => timer.sleep(after).await,
            None => std::future::pending().await,
        }
    };
    let cancelled = async {
        match within {
            Some(exec) => {
                exec.cancelled().await;
                exec.cancel_reason()
            }
            None => std::future::pending().await,
        }
    };
    // Unbiased: when more than one is ready, one is picked at random.
    futures_util::select! {
        out = work.fuse() => out,
        () = expired.fuse() => Err(RpcError::new(ErrorKind::Timeout, "the call timed out")),
        reason = cancelled.fuse() => Err(match reason {
            Some(CancelReason::Deadline) => RpcError::new(ErrorKind::Timeout, "the calling execution's deadline passed"),
            _ => RpcError::new(ErrorKind::Unavailable, "the calling execution was cancelled"),
        }),
    }
}

/// A call answered by one reply, sent when awaited.
#[must_use = "a call is sent when awaited"]
pub struct Call<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
    pub(crate) spec: Spec,
}

impl<Res> Call<Res> {
    /// A header on the call, `h` or the broker's own headers.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    /// This call's timeout in place of the client's.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.spec.timeout = Some(timeout);
        self
    }

    /// Forwards `exec`'s remaining deadline as `deadline-ms` and cancels the call when `exec` is
    /// cancelled.
    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for Call<Res> {
    type Output = Result<Res, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<Res, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut spec = self.spec;
        Box::pin(async move {
            let limit = spec.limit()?;
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            bounded(&*runtime, limit, within.as_ref(), async move { spec.open().await?.single::<Res>().await }).await
        })
    }
}

/// An event, published when awaited.
#[must_use = "an event is published when awaited"]
pub struct Emit {
    pub(crate) spec: Spec,
}

impl Emit {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl IntoFuture for Emit {
    type Output = Result<(), RpcError>;
    type IntoFuture = BoxFuture<'static, Result<(), RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut spec = self.spec;
        Box::pin(async move {
            let limit = spec.limit()?;
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            bounded(&*runtime, limit, within.as_ref(), async move {
                let Spec { client, pattern, request, headers, .. } = spec;
                let data = match request {
                    Request::One(payload) => payload?,
                    Request::Stream(_) => return Err(RpcError::new(ErrorKind::BadRequest, "an event carries one payload")),
                };
                let conn = client.inner.connection().await?;
                let frame = Frame::Evt { pattern: pattern.as_str().to_owned(), headers, data };
                conn.send(&pattern, frame, None).await.map_err(send_error)
            })
            .await
        })
    }
}

/// A call answered by a stream, opened when awaited.
#[must_use = "a call is sent when awaited"]
pub struct StreamCall<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
    pub(crate) spec: Spec,
}

impl<Res> StreamCall<Res> {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    /// The longest wait for each reply frame, the client's timeout unset.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.spec.timeout = Some(timeout);
        self
    }

    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for StreamCall<Res> {
    type Output = Result<RpcStream<Res>, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<RpcStream<Res>, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut spec = self.spec;
        Box::pin(async move {
            let limit = spec.limit()?;
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            let exchange = bounded(&*runtime, limit, within.as_ref(), spec.open()).await?;
            Ok(RpcStream { items: replies::<Res>(exchange, limit) })
        })
    }
}

/// The reply frames as items, each awaited under `per_frame`. The exchange is dropped with the
/// stream, which sends `cancel` unless an `end`, an `err` or a single `res` settled it.
fn replies<Res: DeserializeOwned + Send + 'static>(exchange: Exchange, per_frame: Option<Duration>) -> BoxStream<'static, Result<Res, RpcError>> {
    let items = futures_util::stream::unfold(Some(exchange), move |exchange| async move {
        let mut exchange = exchange?;
        let timer = Arc::clone(&exchange.timer);
        let within = exchange.within.clone();
        let codec = exchange.codec;
        let next = bounded(&*timer, per_frame, within.as_ref(), async { Ok(exchange.replies.next().await) }).await;
        match next {
            Ok(Some(Ok(Frame::Item { data, .. }))) => Some((decode(codec, &data), Some(exchange))),
            // A server whose handler answered one value: the stream is that value.
            Ok(Some(Ok(Frame::Res { data, .. }))) => {
                exchange.pending.settled = true;
                Some((decode(codec, &data), None))
            }
            Ok(Some(Ok(Frame::End { .. }))) => {
                exchange.pending.settled = true;
                None
            }
            Ok(Some(Ok(Frame::Err { error, .. }))) => {
                exchange.pending.settled = true;
                Some((Err(RpcError::from_body(error)), None))
            }
            Ok(Some(Ok(other))) => Some((Err(unexpected(&other)), None)),
            Ok(Some(Err(err))) | Err(err) => Some((Err(err), None)),
            Ok(None) => Some((Err(lost()), None)),
        }
    });
    Box::pin(items)
}

/// A stream of replies: `item` frames decoded as `Res`, ending at `end`, or with one `Err` at an
/// `err`. Dropped before its end, it sends `cancel`.
pub struct RpcStream<Res> {
    pub(crate) items: BoxStream<'static, Result<Res, RpcError>>,
}

impl<Res> Stream for RpcStream<Res> {
    type Item = Result<Res, RpcError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}

/// What a caller sees, uniform across links (transports DESIGN §5.2): the server's `err` with its
/// kind and details, or the link's own failure mapped one way everywhere. No responders, a lost
/// link or a broker refusal is `Unavailable`; the client's timeout `Timeout`; an oversized payload
/// `BadRequest` with `reason: "payload_too_large"`; a binary payload on a JSON link `BadRequest`
/// with `reason: "binary_unsupported"`, before any I/O; a request frame the link cannot encode
/// `Internal`. A pattern nothing handles is `Unavailable`
/// with `reason: "pattern_unhandled"` or `"no_destination"` where the link signals it, and the
/// client's `Timeout` where it cannot.
#[derive(Debug)]
pub struct RpcError {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) details: Details,
}

impl RpcError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        RpcError { kind, message: message.into(), details: Details::new() }
    }

    pub(crate) fn with_reason(mut self, why: &str) -> Self {
        self.details.push(reason(why));
        self
    }

    pub(crate) fn from_body(body: ErrorBody) -> Self {
        RpcError { kind: body.kind, message: body.message, details: body.details }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &Details {
        &self.details
    }

    /// The `ErrorInfo` detail's `reason`, when one is present: `"no_destination"`,
    /// `"pattern_unhandled"`, `"payload_too_large"`, `"binary_unsupported"`.
    pub fn reason(&self) -> Option<&str> {
        self.details.iter().find_map(|detail| match detail {
            Detail::ErrorInfo { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl Error for RpcError {}

impl Classify for RpcError {
    fn classify(&self) -> ErrorKind {
        self.kind
    }

    fn details(&self) -> Details {
        self.details.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{Capabilities, DeliveryMode, Inbound};

    /// A JSON link whose client side encodes its frames as CBOR: a JSON payload's opening `{` is
    /// a CBOR text string 27 bytes long, which a short payload cannot fill.
    struct MisencodingLink;

    impl Link for MisencodingLink {
        const NAME: &'static str = "misencoding";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self) -> Result<Outbound, BoxError> {
            Ok(Outbound {
                send: Box::new(|_pattern, frame, _reply_to| {
                    Box::pin(async move { Codec::Cbor.encode_frame(&frame).map(drop).map_err(BoxError::from) })
                }),
                replies: Box::pin(futures_util::stream::pending()),
            })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// A link whose first connection's reply lane ends at once, as a server closing does, and
    /// whose later ones stay open; it counts its connections.
    struct EndingLink(Arc<AtomicU64>);

    impl Link for EndingLink {
        const NAME: &'static str = "ending";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self) -> Result<Outbound, BoxError> {
            let first = self.0.fetch_add(1, Ordering::Relaxed) == 0;
            let replies: BoxStream<'static, Frame> =
                if first { Box::pin(futures_util::stream::empty()) } else { Box::pin(futures_util::stream::pending()) };
            Ok(Outbound { send: Box::new(|_pattern, _frame, _reply_to| Box::pin(async { Ok(()) })), replies })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// F364: a call handed a connection whose reply lane then ended, before the call registered,
    /// waited for its own timeout, since the lane had already failed every call registered on it.
    #[tokio::test(flavor = "current_thread")]
    async fn a_call_is_not_registered_on_a_connection_whose_reply_lane_ended() {
        let connects = Arc::new(AtomicU64::new(0));
        let link = EndingLink(Arc::clone(&connects));
        let client = RpcClient::of_module(Arc::new(link), Bound::Unbounded, Arc::new(ulo_tokio::Tokio::current()));
        let handed = client.inner.connection().await.expect("the first connection opens");
        // The reply lane's task runs once this task yields, and ends with its empty stream.
        for _ in 0..100 {
            if !handed.open.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!handed.open.load(Ordering::Acquire), "the first connection's reply lane did not end");

        let (waiting, _replies) = mpsc::unbounded();
        assert!(!handed.register(1, waiting), "a call was registered on a connection whose reply lane had ended");
        assert!(handed.pending().is_empty(), "the ended connection holds a call no reply can reach");

        let (conn, _, _, _) = client.inner.register().await.expect("the call registers on a new connection");
        assert!(!Arc::ptr_eq(&conn, &handed), "the call was registered on the ended connection");
        assert_eq!(connects.load(Ordering::Relaxed), 2, "the call did not go over a new connection");
    }

    #[tokio::test]
    async fn a_request_frame_the_link_cannot_encode_is_internal() {
        let client = RpcClient::of_module(Arc::new(MisencodingLink), Bound::Unbounded, Arc::new(ulo_tokio::Tokio::current()));
        let answer = client.request::<_, serde_json::Value>("orders.create", &serde_json::json!({ "id": 1 })).await;
        let err = answer.expect_err("the link refused to encode the request, so the call must fail");
        assert_eq!(err.kind(), ErrorKind::Internal, "expected `Internal` for an unencodable request frame, got: {err:?}");
        assert!(err.message().contains("the Cbor codec cannot encode the frame"), "the message should name the codec's failure, got: {err:?}");
    }
}
