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
//! One task per connection routes the link's reply lane to the calls waiting on it, by `id`. A
//! reply for an id no call waits on any more, a second reply on a `FanOut` link or one arriving
//! after its call gave up, is dropped. A lost reply lane fails every call waiting on it
//! `Unavailable`, and the next call connects again; a `goaway` lets the calls in flight finish and
//! sends the next call over a new connection.

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

use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use ulo::{Bound, BoxError, BoxFuture, CancelReason, ExecutionRef, Timer};
use ulo_transport::{Classify, Detail, Details, ErrorKind};

use crate::codec::Codec;
use crate::dispatch::reason;
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{FrameTooLarge, Link, NoDestination, Outbound, Pattern, ReplyTo};
use crate::transport::CallHeaders;

/// A client's timeout at `Bound::Default`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A client of one link, bound by `RpcClientModule`: `Dep<RpcClient, Billing>` under the module's
/// qualifier. Cheap to clone. Every timeout runs on the app's `Timer`; dropping a pending request
/// or a stream sends `cancel`. There is no ambient execution: a call made inside one forwards its
/// deadline and cancellation with [`Call::within`].
#[derive(Clone)]
pub struct RpcClient {
    pub(crate) inner: Arc<ClientInner>,
}

type Connect = Box<dyn Fn() -> BoxFuture<'static, Result<Outbound, BoxError>> + Send + Sync>;

pub(crate) struct ClientInner {
    link: &'static str,
    connect: Connect,
    codec: Codec,
    /// The module's timeout; `None` for `Bound::Unbounded`.
    timeout: Option<Duration>,
    timer: Arc<dyn Timer>,
    /// Held across `connect`, so concurrent first calls share one connection.
    conn: tokio::sync::Mutex<Option<Arc<Conn>>>,
    next_id: AtomicU64,
}

/// One connection of the link's client side.
struct Conn {
    send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    pending: Mutex<HashMap<u64, Waiting>>,
    /// False once the reply lane ended or the server sent `goaway`: no new call goes over it.
    open: AtomicBool,
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
}

impl RpcClient {
    pub(crate) fn new<L: Link>(link: Arc<L>, timeout: Bound, timer: Arc<dyn Timer>) -> Self {
        let codec = Codec::of(&link.capabilities());
        let timeout = match timeout {
            Bound::Default => Some(DEFAULT_TIMEOUT),
            Bound::After(after) => Some(after),
            Bound::Unbounded => None,
        };
        let connect: Connect = Box::new(move || {
            let link = Arc::clone(&link);
            Box::pin(async move { link.connect().await })
        });
        RpcClient {
            inner: Arc::new(ClientInner {
                link: L::NAME,
                connect,
                codec,
                timeout,
                timer,
                conn: tokio::sync::Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
        }
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
    /// The call's own timeout, in place of the module's.
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

    /// The wait the call is bounded by: its own timeout or the module's, and the calling
    /// execution's remaining time when that is shorter. Stamps `deadline-ms` with that remaining
    /// time; refuses a call whose execution's deadline has passed, before any I/O.
    fn limit(&mut self) -> Result<Option<Duration>, RpcError> {
        let inner = &self.client.inner;
        let own = self.timeout.or(inner.timeout);
        let Some(deadline) = self.within.as_ref().and_then(ExecutionRef::deadline) else {
            return Ok(own);
        };
        let remaining = deadline.saturating_duration_since(inner.timer.now());
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
        let conn = inner.connection().await?;
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (waiting, replies) = mpsc::unbounded_channel();
        conn.pending().insert(id, waiting.clone());
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
                    tokio::spawn(pump(Arc::clone(&conn), pattern, id, items, waiting));
                }
                opened
            }
        };
        if let Err(err) = sent {
            // Nothing reached a server that a `cancel` would stop.
            pending.settled = true;
            return Err(send_error(err));
        }
        Ok(Exchange { replies, pending, codec: inner.codec, timer: Arc::clone(&inner.timer), within })
    }
}

impl ClientInner {
    async fn connection(&self) -> Result<Arc<Conn>, RpcError> {
        let mut slot = self.conn.lock().await;
        if let Some(conn) = slot.as_ref().filter(|conn| conn.open.load(Ordering::Acquire)) {
            return Ok(Arc::clone(conn));
        }
        let Outbound { send, replies } = (self.connect)().await.map_err(|err| {
            RpcError::new(ErrorKind::Unavailable, format!("the {} link could not connect: {err}", self.link))
        })?;
        let conn = Arc::new(Conn { send, pending: Mutex::new(HashMap::new()), open: AtomicBool::new(true) });
        tokio::spawn(route_replies(Arc::downgrade(&conn), replies));
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
            let _ = waiting.send(Ok(frame));
        }
    }
    if let Some(conn) = conn.upgrade() {
        conn.open.store(false, Ordering::Release);
        conn.pending().clear();
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
                let _ = waiting.send(Err(err));
                return;
            }
        };
        if let Err(err) = conn.send(&pattern, Frame::In { id, data }, None).await {
            let _ = waiting.send(Err(send_error(err)));
            return;
        }
    }
    if let Err(err) = conn.send(&pattern, Frame::InEnd { id }, None).await {
        let _ = waiting.send(Err(send_error(err)));
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
        // Outside a runtime there is nothing to send it from; the server's own deadline or the
        // connection's end stops the call instead.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = cancel.await;
            });
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
}

impl Exchange {
    async fn single<Res: DeserializeOwned>(mut self) -> Result<Res, RpcError> {
        match self.replies.recv().await {
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
    } else {
        RpcError::new(ErrorKind::Unavailable, format!("the link refused the frame: {err}"))
    }
}

/// `work` bounded by `limit` on the app's `Timer` and by the calling execution's cancellation.
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
    tokio::select! {
        out = work => out,
        () = expired => Err(RpcError::new(ErrorKind::Timeout, "the call timed out")),
        reason = cancelled => Err(match reason {
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

    /// This call's timeout in place of the module's.
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
            let timer = Arc::clone(&spec.client.inner.timer);
            let within = spec.within.clone();
            bounded(&*timer, limit, within.as_ref(), async move { spec.open().await?.single::<Res>().await }).await
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
            let timer = Arc::clone(&spec.client.inner.timer);
            let within = spec.within.clone();
            bounded(&*timer, limit, within.as_ref(), async move {
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

    /// The longest wait for each reply frame, the module's timeout unset.
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
            let timer = Arc::clone(&spec.client.inner.timer);
            let within = spec.within.clone();
            let exchange = bounded(&*timer, limit, within.as_ref(), spec.open()).await?;
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
        let next = bounded(&*timer, per_frame, within.as_ref(), async { Ok(exchange.replies.recv().await) }).await;
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
/// with `reason: "binary_unsupported"`, before any I/O. A pattern nothing handles is `Unavailable`
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
