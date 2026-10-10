//! One connection: the read loop, the outbound queue, the limits and close codes, keep-alive on the
//! app's `Timer`, the per-message execution, and `on_disconnect` as a terminal execution
//! (transports DESIGN §4.1, §4.2). The handshake and the connection phase are here too, shared by
//! the hand-off on the HTTP server's port and the standalone server.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use async_tungstenite::WebSocketStream;
use async_tungstenite::tungstenite::error::ProtocolError;
use async_tungstenite::tungstenite::handshake::derive_accept_key;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use async_tungstenite::tungstenite::{Error as WsError, Message};
use bytes::Bytes;
use event_listener::{Event, EventListener};
use futures_channel::{mpsc, oneshot};
use futures_core::stream::BoxStream;
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::future::{self, Either};
use futures_util::{FutureExt, SinkExt, StreamExt};
use http::header::{CONNECTION, CONTENT_TYPE, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE, WWW_AUTHENTICATE};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, Closed, DrainToken, ExecOptions, Execution, ExecutionRef, LateOutcome,
    ModuleRef, MountedHandler, Runtime, Spawn, TaskHandle, Timer, Transport,
};
use ulo_transport::{CallError, ErrorKind, TaskSet, Tracked, span};
use ulo_transport::__private::Watch;

use crate::__private::HandlerFn;
use crate::broadcast::NodeId;
use crate::codec::Codec;
use crate::envelope::{self, Frame, Head, MessageId};
use crate::gateway::{
    ConnectRefused, DisconnectReason, GatewayRuntime, GatewaySettings, Instance, Messages, Overflow, Refuse, check_close,
    truncated,
};
use crate::rooms::{BroadcastError, Hub};
use crate::session::SessionHandle;
use crate::transport::{ConnectCx, ConnectInner, ConnectReply, ConnectionInfo, CxInner, NoHandler, Reply, UpgradeHead, Ws, WsConnect, WsCx};

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
    pub(crate) info: ConnectionInfo,
    pub(crate) head: UpgradeHead,
    pub(crate) session: SessionHandle,
    pub(crate) outbound: Arc<Outbound>,
    pub(crate) hub: Arc<Hub>,
    pub(crate) gateway: Arc<GatewayRuntime>,
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) runtime: Arc<dyn Runtime>,
}

impl Connection {
    pub fn id(&self) -> ConnId {
        self.inner.id
    }

    /// Joins `room` on this connection's gateway. Membership is local to this process.
    pub async fn join(&self, room: impl Into<String>) {
        self.inner.hub.join(self.inner.id, room.into());
    }

    pub async fn leave(&self, room: &str) {
        self.inner.hub.leave(self.inner.id, room);
    }

    /// The rooms this connection is in.
    pub fn rooms(&self) -> Vec<String> {
        self.inner.hub.rooms(self.inner.id)
    }

    /// Queues `frame` as one data message, as it stands: no envelope is added. Over
    /// `max_outbound` the connection closes with 1008 "slow consumer", or the oldest queued
    /// message is dropped under `overflow = drop_oldest`; `Err` once the connection has closed.
    pub async fn send(&self, frame: Frame) -> Result<(), BroadcastError> {
        self.inner
            .outbound
            .push(frame.into_message())
            .map_err(|Gone| BroadcastError::new("the connection has closed"))
    }

    /// Closes the connection with `code` and `reason`, which `on_disconnect` receives as
    /// `DisconnectReason::ServerClose`. `code` and `reason` are checked as `ConnectRefused::code`
    /// checks them; an invalid pair closes with 1011.
    ///
    /// The Close frame is written after the messages already queued; the call returns once it is
    /// queued.
    pub async fn close(&self, code: u16, reason: &str) {
        match check_close(code, reason) {
            Ok(()) => self.inner.outbound.request_close(code, reason.to_owned()),
            Err(_) => self.inner.outbound.request_close(1011, String::new()),
        }
    }

    /// An execution in the gateway's module with this connection's inputs seeded:
    /// [`ConnectionInfo`], [`UpgradeHead`] and [`SessionHandle`]. A hand-written gateway runs one
    /// unit of work in it, a graphql-transport-ws `subscribe` for example, and cancels it with
    /// `cancel_with(CancelReason::ClientCancelled)`. Refused once the app's drain has begun.
    pub fn open_execution(&self) -> Result<Execution, Closed> {
        let exec = Execution::open(self.inner.gateway.connect.module(), ExecOptions::new())?;
        self.seed(&exec);
        Ok(exec)
    }

    pub fn info(&self) -> &ConnectionInfo {
        &self.inner.info
    }

    pub fn head(&self) -> &UpgradeHead {
        &self.inner.head
    }

    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// The app's `Timer`, which a hand-written gateway's own clocks read.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.inner.timer
    }

    /// The app's `Runtime`, which a hand-written gateway spawns its own tasks on.
    pub fn runtime(&self) -> &Arc<dyn Runtime> {
        &self.inner.runtime
    }

    pub(crate) fn app(&self) -> &AppHandle {
        &self.inner.app
    }

    /// The connection's three inputs, seeded into `exec`.
    pub(crate) fn seed(&self, exec: &Execution) {
        exec.seed(self.inner.info.clone());
        exec.seed(self.inner.head.clone());
        exec.seed(self.inner.session.clone());
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").field("id", &self.inner.id).finish()
    }
}

/// The connection has closed, or its close is under way.
pub(crate) struct Gone;

/// A connection's outbound queue, written by its message tasks, its broadcasts and
/// `Connection::send`, and drained by its read loop, which owns the socket.
///
/// An `Event` keeps no permit for a waiter that has not registered yet, so each waiter registers
/// its listener before it reads the state it waits on.
pub(crate) struct Outbound {
    state: Mutex<OutState>,
    /// Wakes the read loop to write.
    wake: Event,
    /// Wakes the stream pumps waiting for room.
    space: Event,
    limit: Option<usize>,
    overflow: Overflow,
}

struct OutState {
    queue: VecDeque<Message>,
    /// The close the loop writes after the queue, the first requested.
    close: Option<(u16, String)>,
    closed: bool,
}

impl Outbound {
    pub(crate) fn new(limit: Option<usize>, overflow: Overflow) -> Self {
        Outbound {
            state: Mutex::new(OutState { queue: VecDeque::new(), close: None, closed: false }),
            wake: Event::new(),
            space: Event::new(),
            limit,
            overflow,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `message` under the overflow policy: over the limit, the oldest is dropped, or the
    /// connection closes with 1008 "slow consumer" and what is queued is discarded.
    pub(crate) fn push(&self, message: Message) -> Result<(), Gone> {
        let mut state = self.lock();
        if state.closed || state.close.is_some() {
            return Err(Gone);
        }
        let full = self.limit.is_some_and(|limit| state.queue.len() >= limit);
        if full {
            match self.overflow {
                Overflow::DropOldest => {
                    state.queue.pop_front();
                }
                Overflow::Close => {
                    state.queue.clear();
                    state.close = Some((1008, "slow consumer".to_owned()));
                    drop(state);
                    self.wake.notify(1);
                    return Err(Gone);
                }
            }
        }
        state.queue.push_back(message);
        drop(state);
        self.wake.notify(1);
        Ok(())
    }

    /// Queues `message` once the queue has room. A streamed answer is written at the pace the
    /// client reads it rather than tripping the overflow policy, which governs what the gateway
    /// cannot hold back: broadcasts and `Connection::send`.
    pub(crate) async fn push_wait(&self, message: Message) -> Result<(), Gone> {
        let mut message = Some(message);
        loop {
            let room = self.space.listen();
            {
                let mut state = self.lock();
                if state.closed || state.close.is_some() {
                    return Err(Gone);
                }
                if self.limit.is_none_or(|limit| state.queue.len() < limit) {
                    if let Some(message) = message.take() {
                        state.queue.push_back(message);
                    }
                    drop(state);
                    self.wake.notify(1);
                    return Ok(());
                }
            }
            room.await;
        }
    }

    pub(crate) fn request_close(&self, code: u16, reason: String) {
        let mut state = self.lock();
        if !state.closed && state.close.is_none() {
            state.close = Some((code, truncated(reason)));
        }
        drop(state);
        self.wake.notify(1);
    }

    /// Everything queued, and the close requested after it, if any.
    fn take(&self) -> (VecDeque<Message>, Option<(u16, String)>) {
        let mut state = self.lock();
        let queue = std::mem::take(&mut state.queue);
        let close = state.close.take();
        if close.is_some() {
            state.closed = true;
        }
        drop(state);
        self.space.notify(usize::MAX);
        (queue, close)
    }

    fn shut(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.queue.clear();
        drop(state);
        self.space.notify(usize::MAX);
    }
}

/// Where a server or the hand-off is in its life, as its connections read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Serving,
    Draining,
}

/// The connections one server or the hand-off runs: their tasks, the drain signal they watch,
/// and the drain token their cleanups open terminal executions with.
pub(crate) struct Tracker {
    pub(crate) places: Arc<Places>,
    phase: Watch<Phase>,
    token: Watch<Option<DrainToken>>,
    tasks: Mutex<HashMap<u64, TaskHandle>>,
    next: AtomicU64,
    live: Watch<usize>,
}

impl Tracker {
    /// `max_inflight` is the server's bound on the messages all its connections have in flight
    /// together, `None` for no bound.
    pub(crate) fn new(max_inflight: Option<usize>) -> Arc<Tracker> {
        Arc::new(Tracker {
            places: Arc::new(Places::new(max_inflight)),
            phase: Watch::new(Phase::Serving),
            token: Watch::new(None),
            tasks: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            live: Watch::new(0),
        })
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.phase.read(|phase| *phase == Phase::Draining)
    }

    /// Runs `fut` as a task on `runtime` until it ends or [`close`](Self::close) aborts it.
    pub(crate) fn spawn(self: &Arc<Self>, runtime: &dyn Spawn, fut: impl Future<Output = ()> + Send + 'static) {
        let key = self.next.fetch_add(1, Ordering::Relaxed);
        self.live.modify(|live| *live += 1);
        let guard = TaskGuard { tracker: Arc::clone(self), key };
        // Spawned under the lock, so a task that ends at once removes its entry only after the
        // entry is in; a runtime's `spawn` does not poll the task before it returns.
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        let task = runtime.spawn(Box::pin(async move {
            let _guard = guard;
            fut.await;
        }));
        tasks.insert(key, task);
    }

    /// Raises the drain: idle connections close with 1001 at once, busy ones stop reading,
    /// finish their messages and then close. Returns once every connection has ended.
    pub(crate) async fn drain(&self, token: DrainToken) {
        self.token.modify(|slot| *slot = Some(token));
        self.phase.modify(|phase| *phase = Phase::Draining);
        self.live.wait_for(|live| *live == 0).await;
    }

    /// Aborts every connection left and returns once their tasks have ended.
    pub(crate) async fn close(&self) {
        // Taken out of the lock before aborting: an abort may drop a task's future on this
        // thread, and its guard takes the same lock.
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        for task in tasks.values() {
            task.abort();
        }
        drop(tasks);
        self.live.wait_for(|live| *live == 0).await;
    }

    /// The drain's token, once the drain has begun.
    async fn token(&self) -> Option<DrainToken> {
        self.token.wait_for(Option::is_some).await;
        self.token.read(Clone::clone)
    }
}

/// The places in flight one server's connections share, under its `server_max_inflight`. A
/// message takes one when its handler is to run and gives it back once its answer is known, as it
/// counts under its own connection's `max_inflight`. A connection reads nothing while every place
/// is taken, and a message it read as the last place went waits, unhandled, for one.
pub(crate) struct Places {
    held: AtomicUsize,
    limit: usize,
    /// Wakes the connections waiting for a place, when one frees with none left.
    freed: Event,
}

impl Places {
    fn new(limit: Option<usize>) -> Places {
        Places { held: AtomicUsize::new(0), limit: limit.unwrap_or(usize::MAX), freed: Event::new() }
    }

    fn is_full(&self) -> bool {
        self.held.load(Ordering::Acquire) >= self.limit
    }

    fn try_take(&self) -> bool {
        self.held.fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| (held < self.limit).then_some(held + 1)).is_ok()
    }

    fn give_back(&self, places: usize) {
        if places == 0 {
            return;
        }
        let before = self.held.fetch_sub(places, Ordering::AcqRel);
        if before >= self.limit {
            self.freed.notify(usize::MAX);
        }
    }
}

struct TaskGuard {
    tracker: Arc<Tracker>,
    key: u64,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.tracker.tasks.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.key);
        self.tracker.live.modify(|live| *live = live.saturating_sub(1));
    }
}

/// What a connection on one gateway needs from the server that accepted it.
#[derive(Clone)]
pub(crate) struct Accept {
    pub(crate) gateway: Arc<GatewayRuntime>,
    pub(crate) hub: Arc<Hub>,
    pub(crate) app: AppHandle,
    /// The app's runtime: every task a connection starts, the connection's own included, runs
    /// on it, and its `Timer` half keeps the connection's clocks.
    pub(crate) runtime: Arc<dyn Runtime>,
    pub(crate) tracker: Arc<Tracker>,
}

/// A server's answer to one upgrade request, from
/// [`GatewayTable::handshake`](crate::GatewayTable::handshake).
pub enum Handshake {
    /// Write [`Switch::response`], the 101, then hand the upgraded stream to [`Switch::serve`].
    Switch(Switch),
    /// Write [`Refusal::into_response`]; the request is not upgraded.
    Refuse(Refusal),
}

/// An upgrade request the handshake refused: its status, a reason for the body, and the headers
/// the status calls for (`Allow` on a 405, `Sec-WebSocket-Version` on a 426, `WWW-Authenticate`
/// on a 401).
#[derive(Debug)]
pub struct Refusal {
    status: StatusCode,
    reason: String,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl Refusal {
    pub(crate) fn new(status: StatusCode, reason: impl Into<String>) -> Refusal {
        Refusal { status, reason: reason.into(), headers: Vec::new() }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The response to write: the status, the reason as a `text/plain; charset=utf-8` body, and
    /// the refusal's headers.
    pub fn into_response(self) -> http::Response<String> {
        let mut response = http::Response::new(self.reason);
        *response.status_mut() = self.status;
        response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        for (name, value) in self.headers {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

/// An upgrade request the handshake accepted, with what the connection needs once upgraded.
///
/// Under `refuse = handshake` the connection phase has run by the time a `Switch` exists: the
/// connect guards admitted the connection, and then either it took a slot under
/// `max_connections` and `OnConnect` ran, or no slot was free and no hook ran, the connection to
/// be closed with 1013 once upgraded. Dropped without [`serve`](Self::serve), as when writing the
/// 101 fails, a connection whose `OnConnect` ran gets `on_disconnect` with
/// `DisconnectReason::Lost` and then leaves the rooms, on a task the table's drain and close reach.
/// Under `refuse = close` nothing has run, and a dropped `Switch` runs nothing.
pub struct Switch {
    accept: Accept,
    head: Arc<Parts>,
    peer: Option<SocketAddr>,
    protocol: Option<Arc<str>>,
    headers: Vec<(HeaderName, HeaderValue)>,
    /// What the connection phase decided, taken by `serve`.
    decided: Decided,
}

/// What the handshake's connection phase decided about a connection it switches.
enum Decided {
    /// `refuse = close`: the phase runs once the connection is upgraded.
    Later,
    /// Admitted with a slot, `OnConnect` having run.
    Admitted(Admitted),
    /// Admitted by the connect guards with no slot free: closed with 1013 once upgraded, no hook
    /// having run.
    Full,
}

impl Switch {
    /// The 101 to write: `Upgrade`, `Connection`, `Sec-WebSocket-Accept` and the subprotocol
    /// chosen, if any. No extension is negotiated.
    pub fn response(&self) -> http::Response<()> {
        let mut response = http::Response::new(());
        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        response.headers_mut().extend(self.headers.iter().cloned());
        response
    }

    /// The connection driver: once `upgraded` yields the upgraded stream, runs the gateway on it
    /// until the connection ends, as a task on the app's runtime that the table's drain and close
    /// reach. The connection runs the connection phase unless the handshake ran it: the connect
    /// guards, then a slot under `max_connections` (closing with 1013, no hook having run, when
    /// none is free), then `OnConnect`. It then reads messages until it closes, and
    /// `on_disconnect` runs for a connection whose `OnConnect` ran.
    ///
    /// `upgraded` may resolve only once the 101 has been written, as hyper's upgrade does; a
    /// server holding the stream already passes `async move { Ok(io) }`. An `Err` ends the
    /// connection before it opens, logged at `debug`; a connection the handshake's connection
    /// phase admitted then gets `on_disconnect` with `DisconnectReason::Lost`.
    pub fn serve<Io, U>(mut self, upgraded: U)
    where
        U: Future<Output = Result<Io, BoxError>> + Send + 'static,
        Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let decided = std::mem::replace(&mut self.decided, Decided::Later);
        let accept = self.accept.clone();
        let tracker = Arc::clone(&accept.tracker);
        let runtime = Arc::clone(&accept.runtime);
        let (head, peer, protocol) = (Arc::clone(&self.head), self.peer, self.protocol.clone());
        tracker.spawn(&*runtime, run(accept, head, peer, protocol, upgraded, decided));
    }
}

impl Drop for Switch {
    fn drop(&mut self) {
        if let Decided::Admitted(admitted) = std::mem::replace(&mut self.decided, Decided::Later) {
            let accept = self.accept.clone();
            self.accept.tracker.spawn(&*self.accept.runtime, async move {
                admitted.end(&accept, DisconnectReason::Lost).await;
            });
        }
    }
}

/// Decides one upgrade request on `accept`'s gateway: the RFC 6455 handshake checks, the
/// subprotocol negotiation, and the connection phase before the 101 under `refuse = handshake`.
///
/// The connection phase runs as a task the table's drain and close reach, and the decision waits
/// for its answer. A server that drops the decision while `OnConnect` runs, as hyper drops a
/// request whose client has gone, leaves the phase to finish; the `Switch` nobody receives then
/// ends the connection it admitted through its `Drop`.
pub(crate) async fn handshake(accept: Accept, head: Parts, peer: Option<SocketAddr>) -> Handshake {
    let checked = match check_handshake(&head, accept.gateway.settings()) {
        Ok(checked) => checked,
        Err(refusal) => return Handshake::Refuse(refusal),
    };
    if accept.tracker.is_draining() {
        return Handshake::Refuse(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "the server is shutting down"));
    }
    let head = Arc::new(head);
    let protocol: Option<Arc<str>> = checked.protocol.as_deref().map(Arc::from);
    match accept.gateway.settings().refuse {
        Refuse::Close => Handshake::Switch(Switch { accept, head, peer, protocol, headers: checked.headers, decided: Decided::Later }),
        Refuse::Handshake => {
            let (answer, answered) = oneshot::channel();
            let (tracker, runtime) = (Arc::clone(&accept.tracker), Arc::clone(&accept.runtime));
            tracker.spawn(&*runtime, async move {
                let decided = connection_phase(accept, head, peer, protocol, checked.headers).await;
                // An answer nobody receives is dropped here, or with the channel.
                let _ = answer.send(decided);
            });
            // Cancelled only when the table's close aborts the phase.
            answered.await.unwrap_or_else(|_| Handshake::Refuse(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "the server is shutting down")))
        }
    }
}

/// The connection phase before the 101: a `Switch` carrying the admitted connection, or the
/// connection the guards admitted with no slot free, or the phase's refusal as a 401 or 403.
async fn connection_phase(
    accept: Accept,
    head: Arc<Parts>,
    peer: Option<SocketAddr>,
    protocol: Option<Arc<str>>,
    headers: Vec<(HeaderName, HeaderValue)>,
) -> Handshake {
    match connect(&accept, Arc::clone(&head), peer, protocol.clone()).await {
        Connected::Admitted(admitted) => {
            Handshake::Switch(Switch { accept, head, peer, protocol, headers, decided: Decided::Admitted(admitted) })
        }
        Connected::Full => Handshake::Switch(Switch { accept, head, peer, protocol, headers, decided: Decided::Full }),
        Connected::Refused(refused) => {
            let mut headers = Vec::new();
            if refused.status() == StatusCode::UNAUTHORIZED {
                headers.push((WWW_AUTHENTICATE, HeaderValue::from_static("Bearer")));
            }
            Handshake::Refuse(Refusal { status: refused.status(), reason: refused.reason().to_owned(), headers })
        }
    }
}

/// The connection after its 101: the connection phase unless the handshake ran it, the read
/// loop, then `on_disconnect` for a connection whose `OnConnect` ran.
async fn run<Io, U>(
    accept: Accept,
    head: Arc<Parts>,
    peer: Option<SocketAddr>,
    protocol: Option<Arc<str>>,
    upgrade: U,
    decided: Decided,
) where
    U: Future<Output = Result<Io, BoxError>> + Send + 'static,
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = match upgrade.await {
        Ok(io) => io,
        Err(error) => {
            tracing::debug!(%error, peer = ?peer, "a WebSocket upgrade did not complete");
            if let Decided::Admitted(admitted) = decided {
                admitted.end(&accept, DisconnectReason::Lost).await;
            }
            return;
        }
    };
    let limits = &accept.gateway.limits;
    let config = WebSocketConfig::default()
        .max_message_size(Some(limits.message_limit))
        .max_frame_size(Some(limits.frame_limit));
    let mut ws = WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await;
    let admitted = match decided {
        Decided::Admitted(admitted) => admitted,
        Decided::Full => return refuse(&mut ws, &accept, 1013, TOO_MANY).await,
        Decided::Later => match connect(&accept, head, peer, protocol).await {
            Connected::Admitted(admitted) => admitted,
            Connected::Full => return refuse(&mut ws, &accept, 1013, TOO_MANY).await,
            Connected::Refused(refused) => return refuse(&mut ws, &accept, refused.close_code(), refused.reason()).await,
        },
    };
    let why = serve(ws, &admitted, &accept).await;
    admitted.end(&accept, why).await;
}

/// The close reason of a connection over `max_connections`.
const TOO_MANY: &str = "too many connections";

/// A connection's place under its gateway's `max_connections`.
struct Slot(Arc<GatewayRuntime>);

impl Slot {
    fn acquire(gateway: &Arc<GatewayRuntime>) -> Option<Slot> {
        let limit = gateway.limits.max_connections.unwrap_or(usize::MAX);
        gateway
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| (live < limit).then_some(live + 1))
            .ok()
            .map(|_| Slot(Arc::clone(gateway)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Checked {
    headers: Vec<(HeaderName, HeaderValue)>,
    protocol: Option<String>,
}

/// RFC 6455 §4.2.1's checks on the client's handshake, and the 101's headers: the accept key and
/// the first of the gateway's subprotocols the client offered. No extension is negotiated, so a
/// client offering `permessage-deflate` is answered without `Sec-WebSocket-Extensions`
/// (§9.1).
fn check_handshake(head: &Parts, settings: &GatewaySettings) -> Result<Checked, Refusal> {
    if head.method != Method::GET {
        return Err(Refusal {
            status: StatusCode::METHOD_NOT_ALLOWED,
            reason: "a WebSocket handshake is a GET".to_owned(),
            headers: vec![(http::header::ALLOW, HeaderValue::from_static("GET"))],
        });
    }
    if head.version < Version::HTTP_11 {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, "a WebSocket handshake is HTTP/1.1"));
    }
    if !has_token(&head.headers, &CONNECTION, "upgrade") || !has_token(&head.headers, &UPGRADE, "websocket") {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, "a WebSocket handshake carries `Connection: Upgrade` and `Upgrade: websocket`"));
    }
    if head.headers.get(SEC_WEBSOCKET_VERSION).map(HeaderValue::as_bytes) != Some(b"13".as_slice()) {
        return Err(Refusal {
            status: StatusCode::UPGRADE_REQUIRED,
            reason: "the WebSocket version is 13".to_owned(),
            headers: vec![(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"))],
        });
    }
    let Some(key) = head.headers.get(SEC_WEBSOCKET_KEY).filter(|key| !key.is_empty()) else {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, "a WebSocket handshake carries `Sec-WebSocket-Key`"));
    };
    let offered: Vec<&str> = head
        .headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    let protocol = settings.subprotocols.iter().find(|name| offered.iter().any(|offer| *offer == &***name)).map(|name| name.to_string());
    let mut headers = vec![
        (UPGRADE, HeaderValue::from_static("websocket")),
        (CONNECTION, HeaderValue::from_static("Upgrade")),
    ];
    match HeaderValue::from_str(&derive_accept_key(key.as_bytes())) {
        Ok(accept) => headers.push((SEC_WEBSOCKET_ACCEPT, accept)),
        Err(_) => return Err(Refusal::new(StatusCode::BAD_REQUEST, "`Sec-WebSocket-Key` is malformed")),
    }
    if let Some(value) = protocol.as_deref().and_then(|name| HeaderValue::from_str(name).ok()) {
        headers.push((SEC_WEBSOCKET_PROTOCOL, value));
    }
    Ok(Checked { headers, protocol })
}

/// Whether a comma-separated header `name` lists `token`, ignoring case.
fn has_token(headers: &HeaderMap, name: &HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|item| item.trim().eq_ignore_ascii_case(token))
}

/// A connection the connection phase admitted, with its slot under `max_connections` and a
/// hand-written gateway's instance.
struct Admitted {
    conn: Connection,
    instance: Option<Instance>,
    slot: Slot,
}

impl Admitted {
    /// `on_disconnect` with `why`, then out of the rooms, then the slot freed: how every admitted
    /// connection ends, served or not, so a connection that ran `OnConnect` always gets
    /// `on_disconnect`.
    async fn end(self, accept: &Accept, why: DisconnectReason) {
        disconnect(&self.conn, why, accept).await;
        accept.hub.unregister(self.conn.id());
        drop(self.slot);
    }
}

enum Connected {
    Admitted(Admitted),
    /// The connect guards admitted the connection and no slot was free; no hook ran.
    Full,
    Refused(ConnectRefused),
}

/// The connection phase: one execution in the gateway's module with the connection's inputs
/// seeded, the session built in it before anything else runs, then `dispatch` of the connect
/// handler: its connect guards, then a slot under `max_connections`, then its `OnConnect`. The
/// slot is taken where the guards have admitted and no hook has run, as the handler's call
/// begins, so a guard's refusal holds no slot and a connection refused for capacity runs no
/// `OnConnect` and needs no `on_disconnect`. An error no error handler claims refuses the
/// connection by its kind.
async fn connect(accept: &Accept, head: Arc<Parts>, peer: Option<SocketAddr>, protocol: Option<Arc<str>>) -> Connected {
    let gateway = &accept.gateway;
    let connect = &gateway.connect;
    let Ok(exec) = Execution::open(connect.module(), ExecOptions::new()) else {
        return Connected::Refused(ConnectRefused { close_code: 1001, reason: "the server is shutting down".to_owned(), kind: None });
    };
    let id = accept.hub.next_id();
    let info = ConnectionInfo { peer, path: Arc::clone(&gateway.path), id };
    let head = UpgradeHead { parts: head, subprotocol: protocol };
    exec.seed(info.clone());
    exec.seed(head.clone());
    let built = {
        let resolver = exec.resolver();
        gateway.handler.session.build(&resolver).await
    };
    let value = match built {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, gateway = %gateway.path, "a connection's session could not be built; the connection is refused");
            return Connected::Refused(ConnectRefused::kind(ErrorKind::Internal, "internal error"));
        }
    };
    let session = SessionHandle { conn: id, value };
    exec.seed(session.clone());
    let outbound = Arc::new(Outbound::new(gateway.limits.max_outbound, gateway.settings().overflow));
    let conn = Connection {
        inner: Arc::new(ConnInner {
            id,
            info,
            head,
            session,
            outbound: Arc::clone(&outbound),
            hub: Arc::clone(&accept.hub),
            gateway: Arc::clone(gateway),
            app: accept.app.clone(),
            timer: Arc::clone(&accept.runtime) as Arc<dyn Timer>,
            runtime: Arc::clone(&accept.runtime),
        }),
    };
    accept.hub.register(id, Arc::clone(gateway), outbound);
    let handle = exec.handle();
    let cx = ConnectCx { inner: Arc::new(ConnectInner { exec: handle.clone(), conn: conn.clone() }) };
    let handler_name = format!("{}::connect", gateway.controller);
    let call_span = span::call(<WsConnect as Transport>::KEY, "ws.connect", Some(&handler_name));
    call_span.record(span::URL_PATH, &*gateway.path);
    let admit = Arc::clone(&gateway.handler.admit);
    let held: Arc<Mutex<Option<Slot>>> = Arc::default();
    let full = Arc::new(AtomicBool::new(false));
    let call = {
        let (gateway, held, full) = (Arc::clone(gateway), Arc::clone(&held), Arc::clone(&full));
        move |cx: ConnectCx| -> BoxFuture<'static, Result<ConnectReply, BoxError>> {
            match Slot::acquire(&gateway) {
                Some(slot) => {
                    *held.lock().unwrap_or_else(PoisonError::into_inner) = Some(slot);
                    admit(cx)
                }
                None => {
                    full.store(true, Ordering::Release);
                    Box::pin(future::ready(Ok(ConnectReply::Refused(ConnectRefused::kind(ErrorKind::Unavailable, TOO_MANY)))))
                }
            }
        }
    };
    let outcome = ulo::dispatch(connect, &handle, &cx, call).instrument(call_span).await;
    let slot = held.lock().unwrap_or_else(PoisonError::into_inner).take();
    if full.load(Ordering::Acquire) {
        accept.hub.unregister(id);
        return Connected::Full;
    }
    let reply = outcome.unwrap_or_else(|err| ConnectReply::Refused(ConnectRefused::of_error(err)));
    let refused = match reply {
        // An interceptor answering for the handler admits without the call that takes the slot,
        // so the slot is taken here, no hook having run.
        ConnectReply::Admitted => match slot.or_else(|| Slot::acquire(gateway)) {
            None => {
                accept.hub.unregister(id);
                return Connected::Full;
            }
            Some(slot) => match &gateway.handler.messages {
                Messages::Envelope => return Connected::Admitted(Admitted { conn, instance: None, slot }),
                Messages::Raw { instance, .. } => match instance(handle).await {
                    Ok(instance) => return Connected::Admitted(Admitted { conn, instance: Some(instance), slot }),
                    Err(error) => {
                        tracing::error!(%error, gateway = %gateway.path, "a hand-written gateway could not be resolved for a connection");
                        ConnectRefused::kind(ErrorKind::Internal, "internal error")
                    }
                },
            },
        },
        ConnectReply::Refused(refused) => refused,
    };
    accept.hub.unregister(id);
    Connected::Refused(refused)
}

/// A Close frame with `code` and `reason`, then a wait for the client's, bounded by
/// `pong_timeout`, the time a peer is given to answer a control frame.
async fn refuse<Io>(ws: &mut WebSocketStream<Io>, accept: &Accept, code: u16, reason: &str)
where
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if send_close(ws, code, reason).await.is_err() {
        return;
    }
    let mut deadline = accept.gateway.limits.pong_timeout.map(|after| accept.runtime.sleep(after));
    let mut turn = 0usize;
    loop {
        // Each turn polls the other branch first, so neither is favoured: a client that keeps
        // sending cannot hold the wait past its deadline.
        let first = turn;
        turn = turn.wrapping_add(1);
        let next = poll_fn(|cx| {
            for branch in [first, first.wrapping_add(1)] {
                if branch % 2 == 0 {
                    if let Poll::Ready(next) = ws.poll_next_unpin(cx) {
                        return Poll::Ready(Some(next));
                    }
                } else if poll_sleep(&mut deadline, cx).is_ready() {
                    return Poll::Ready(None);
                }
            }
            Poll::Pending
        })
        .await;
        match next {
            None | Some(None | Some(Err(_))) => return,
            Some(Some(Ok(Message::Close(_)))) => {
                let _ = ws.flush().await;
            }
            Some(Some(Ok(_))) => {}
        }
    }
}

async fn send_close<Io>(ws: &mut WebSocketStream<Io>, code: u16, reason: &str) -> Result<(), WsError>
where
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let frame = CloseFrame { code: CloseCode::from(code), reason: truncated(reason.to_owned()).into() };
    ws.close(Some(frame)).await
}

/// Ready when `sleep` has ended; pending forever when there is none.
fn poll_sleep(sleep: &mut Option<BoxFuture<'static, ()>>, cx: &mut Context<'_>) -> Poll<()> {
    match sleep {
        Some(sleep) => sleep.as_mut().poll(cx),
        None => Poll::Pending,
    }
}

/// Ready, and spent, once `listener` has been notified.
fn poll_listener(listener: &mut Option<EventListener>, cx: &mut Context<'_>) -> Poll<()> {
    let Some(heard) = listener else { return Poll::Pending };
    if Pin::new(heard).poll(cx).is_pending() {
        return Poll::Pending;
    }
    *listener = None;
    Poll::Ready(())
}

/// A connection's messages in flight, shared with their tasks.
struct Flight {
    /// The server's places, one held for each message counted in `handling`.
    places: Arc<Places>,
    /// Messages a handler is still answering, or a hand-written gateway's messages queued or in
    /// `on_message`: over `max_inflight`, the loop stops reading.
    handling: AtomicUsize,
    /// Messages not yet finished, a streamed answer still writing included: the drain waits for
    /// them.
    busy: AtomicUsize,
    /// Wakes the read loop, which registers before it reads the counts.
    progress: Event,
    calls: Mutex<HashMap<u64, (Option<MessageId>, ExecutionRef)>>,
    next: AtomicU64,
}

impl Flight {
    fn new(places: Arc<Places>) -> Flight {
        Flight {
            places,
            handling: AtomicUsize::new(0),
            busy: AtomicUsize::new(0),
            progress: Event::new(),
            calls: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        }
    }

    /// Counts one message in, with a place of the server's; `false`, counting nothing, when every
    /// place is taken.
    #[must_use]
    fn begin(&self) -> bool {
        if !self.places.try_take() {
            return false;
        }
        self.handling.fetch_add(1, Ordering::AcqRel);
        self.busy.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn handled(&self) {
        self.handling.fetch_sub(1, Ordering::AcqRel);
        self.places.give_back(1);
        self.progress.notify(1);
    }

    fn finished(&self) {
        self.busy.fetch_sub(1, Ordering::AcqRel);
        self.progress.notify(1);
    }

    fn handling(&self) -> usize {
        self.handling.load(Ordering::Acquire)
    }

    fn idle(&self) -> bool {
        self.busy.load(Ordering::Acquire) == 0
    }

    fn register(&self, id: Option<MessageId>, exec: ExecutionRef) -> u64 {
        let key = self.next.fetch_add(1, Ordering::Relaxed);
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).insert(key, (id, exec));
        key
    }

    fn unregister(&self, key: u64) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
    }

    /// `ClientCancelled` on every message in flight with `id`.
    fn cancel(&self, id: &MessageId) {
        for (call, exec) in self.calls.lock().unwrap_or_else(PoisonError::into_inner).values() {
            if call.as_ref() == Some(id) {
                exec.cancel_with(CancelReason::ClientCancelled);
            }
        }
    }

    fn disconnected(&self) {
        for (_, exec) in self.calls.lock().unwrap_or_else(PoisonError::into_inner).values() {
            exec.cancel_with(CancelReason::Disconnected);
        }
    }
}

/// A hand-written gateway's messages still queued when its connection ends are never counted out
/// of `handling`; their places go back with the connection.
impl Drop for Flight {
    fn drop(&mut self) {
        self.places.give_back(*self.handling.get_mut());
    }
}

/// Counts one message down when its task ends, an aborted task included.
struct Finished(Arc<Flight>);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.finished();
    }
}

/// Counts one message out of `handling` once, when its answer is known or its task ends.
struct Handling(Option<Arc<Flight>>);

impl Handling {
    fn done(&mut self) {
        if let Some(flight) = self.0.take() {
            flight.handled();
        }
    }
}

impl Drop for Handling {
    fn drop(&mut self) {
        self.done();
    }
}

/// The read loop: data messages to their handlers or to `on_message`, control frames answered,
/// the outbound queue written, keep-alive, the drain, and the close handshake. Returns why the
/// connection ended.
///
/// tungstenite queues the Pong for a Ping and the reply to a Close, and writes them only when the
/// connection is next polled; the loop stops reading under `max_inflight` and in the drain, so it
/// flushes right after reading either rather than waiting for a later read to carry them out.
async fn serve<Io>(mut ws: WebSocketStream<Io>, admitted: &Admitted, accept: &Accept) -> DisconnectReason
where
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let conn = &admitted.conn;
    let gateway = &accept.gateway;
    let limits = &gateway.limits;
    let outbound = Arc::clone(&conn.inner.outbound);
    let flight = Arc::new(Flight::new(Arc::clone(&accept.tracker.places)));
    let places = &accept.tracker.places;
    let mut tasks = TaskSet::new(Arc::clone(&accept.runtime) as Arc<dyn Spawn>);
    let inbox = match (&gateway.handler.messages, &admitted.instance) {
        (Messages::Raw { on_message, .. }, Some(instance)) => {
            Some(raw_inbox(&mut tasks, conn.clone(), Arc::clone(instance), Arc::clone(on_message), Arc::clone(&flight)))
        }
        _ => None,
    };
    let mut draining = false;
    let mut closing: Option<DisconnectReason> = None;
    let mut close_deadline: Option<BoxFuture<'static, ()>> = None;
    let mut ping = limits.ping_interval.map(|every| accept.runtime.sleep(every));
    let mut pong: Option<BoxFuture<'static, ()>> = None;
    // Each listener is registered before the state it watches is read, kept until it is heard,
    // and registered again on the next turn.
    let mut woken: Option<EventListener> = None;
    let mut progressed: Option<EventListener> = None;
    let mut phase_changed: Option<EventListener> = None;
    let mut freed: Option<EventListener> = None;
    // A message read as the server's last place went, waiting for one.
    let mut waiting: Option<Frame> = None;
    let mut turn = 0usize;

    let why = loop {
        woken.get_or_insert_with(|| outbound.wake.listen());
        progressed.get_or_insert_with(|| flight.progress.listen());
        if !draining {
            phase_changed.get_or_insert_with(|| accept.tracker.phase.changed());
            draining = accept.tracker.is_draining();
        }
        let (queued, close) = outbound.take();
        if write(&mut ws, queued).await.is_err() {
            break closing.unwrap_or(DisconnectReason::Lost);
        }
        let close = match close {
            Some(close) if closing.is_none() => Some(close),
            _ if draining && closing.is_none() && flight.idle() => Some((1001, "the server is shutting down".to_owned())),
            _ => None,
        };
        if let Some((code, reason)) = close {
            let why = if code == 1001 && draining { DisconnectReason::Drain } else { DisconnectReason::ServerClose { code } };
            outbound.shut();
            if send_close(&mut ws, code, &reason).await.is_err() {
                break why;
            }
            closing = Some(why);
            close_deadline = limits.pong_timeout.map(|after| accept.runtime.sleep(after));
        }
        if waiting.is_some() || places.is_full() {
            freed.get_or_insert_with(|| places.freed.listen());
        }
        if closing.is_none()
            && !places.is_full()
            && let Some(frame) = waiting.take()
        {
            waiting = received(frame, conn, &flight, &mut tasks, inbox.as_ref());
        }
        let reading = closing.is_some()
            || (!draining && waiting.is_none() && !places.is_full() && flight.handling() < limits.max_inflight);
        let joining = !tasks.is_empty();
        // Each turn starts at the next branch, so a branch that is always ready cannot starve the
        // others. Every branch's future is dropped before its event is handled.
        let first = turn;
        turn = turn.wrapping_add(1);
        let event = {
            let mut joined = pin!(tasks.join_next());
            poll_fn(|cx| {
                for step in 0..BRANCHES {
                    let ready = match (first.wrapping_add(step)) % BRANCHES {
                        0 if reading => ws.poll_next_unpin(cx).map(Turn::Read),
                        1 => poll_listener(&mut woken, cx).map(|()| Turn::Woken),
                        2 => poll_listener(&mut progressed, cx).map(|()| Turn::Woken),
                        3 if !draining => poll_listener(&mut phase_changed, cx).map(|()| Turn::Woken),
                        4 if closing.is_none() => poll_sleep(&mut ping, cx).map(|()| Turn::Ping),
                        5 => poll_sleep(&mut pong, cx).map(|()| Turn::PongMissed),
                        6 => poll_sleep(&mut close_deadline, cx).map(|()| Turn::CloseDeadline),
                        7 if joining => joined.as_mut().poll(cx).map(|_| Turn::Woken),
                        8 => poll_listener(&mut freed, cx).map(|()| Turn::Woken),
                        _ => Poll::Pending,
                    };
                    if ready.is_ready() {
                        return ready;
                    }
                }
                Poll::Pending
            })
            .await
        };
        match event {
            Turn::Read(next) => match next {
                None => break closing.unwrap_or(DisconnectReason::Lost),
                Some(Ok(Message::Text(text))) if closing.is_none() => {
                    let frame = Frame::Text(text.as_str().to_owned());
                    waiting = received(frame, conn, &flight, &mut tasks, inbox.as_ref());
                }
                Some(Ok(Message::Binary(bytes))) if closing.is_none() => {
                    waiting = received(Frame::Binary(bytes), conn, &flight, &mut tasks, inbox.as_ref());
                }
                Some(Ok(Message::Ping(_))) => {
                    if ws.flush().await.is_err() {
                        break closing.unwrap_or(DisconnectReason::Lost);
                    }
                }
                Some(Ok(Message::Pong(_))) => pong = None,
                Some(Ok(Message::Close(frame))) => {
                    let _ = ws.flush().await;
                    if closing.is_none() {
                        outbound.shut();
                        let (code, reason) = match frame {
                            Some(frame) => (u16::from(frame.code), frame.reason.as_str().to_owned()),
                            None => (1005, String::new()),
                        };
                        closing = Some(DisconnectReason::ClientClose { code, reason });
                        close_deadline = limits.pong_timeout.map(|after| accept.runtime.sleep(after));
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    let (code, why) = read_failure(&error);
                    if let Some(code) = code {
                        outbound.shut();
                        let _ = send_close(&mut ws, code, "").await;
                    }
                    break match closing {
                        Some(closing) if why == DisconnectReason::Lost => closing,
                        _ => why,
                    };
                }
            },
            Turn::Woken => {}
            Turn::Ping => {
                if ws.send(Message::Ping(Bytes::new())).await.is_err() {
                    break DisconnectReason::Lost;
                }
                ping = limits.ping_interval.map(|every| accept.runtime.sleep(every));
                if pong.is_none() {
                    pong = limits.pong_timeout.map(|after| accept.runtime.sleep(after));
                }
            }
            Turn::PongMissed => break DisconnectReason::Lost,
            Turn::CloseDeadline => break closing.clone().unwrap_or(DisconnectReason::Lost),
        }
    };
    outbound.shut();
    flight.disconnected();
    drop(inbox);
    tasks.abort_all();
    tasks.join_all().await;
    why
}

/// How many branches the read loop's wait polls.
const BRANCHES: usize = 9;

/// What ended one turn of the read loop's wait.
enum Turn {
    Read(Option<Result<Message, WsError>>),
    /// A queued message or close, a message's progress, the drain, or a message task's end:
    /// the next turn reads the state again.
    Woken,
    Ping,
    PongMissed,
    CloseDeadline,
}

/// The close code to send, if any, and the reason, for a read error. A connection reset without
/// a Close frame is `Lost`, as an I/O error is; a protocol, UTF-8 or capacity error is
/// `ProtocolError` and closes with 1002, 1007 or 1009.
fn read_failure(error: &WsError) -> (Option<u16>, DisconnectReason) {
    match error {
        WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => (None, DisconnectReason::Lost),
        WsError::Protocol(_) => (Some(1002), DisconnectReason::ProtocolError),
        WsError::Utf8(_) => (Some(1007), DisconnectReason::ProtocolError),
        WsError::Capacity(_) => (Some(1009), DisconnectReason::ProtocolError),
        _ => (None, DisconnectReason::Lost),
    }
}

async fn write<Io>(ws: &mut WebSocketStream<Io>, queued: VecDeque<Message>) -> Result<(), WsError>
where
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if queued.is_empty() {
        return Ok(());
    }
    for message in queued {
        ws.feed(message).await?;
    }
    ws.flush().await
}

/// One data message: to a hand-written gateway's inbox, or through the envelope to its handler.
/// A frame of the kind the gateway's codec does not read closes the connection with 1003. A
/// message to be handled with every place of the server's taken is answered back, to wait for one;
/// a `cancel` and a frame refused before dispatch take no place.
fn received(
    frame: Frame,
    conn: &Connection,
    flight: &Arc<Flight>,
    tasks: &mut TaskSet,
    inbox: Option<&mpsc::UnboundedSender<Frame>>,
) -> Option<Frame> {
    if let Some(inbox) = inbox {
        if !flight.begin() {
            return Some(frame);
        }
        if inbox.unbounded_send(frame).is_err() {
            flight.handled();
            flight.finished();
        }
        return None;
    }
    let gateway = &conn.inner.gateway;
    let codec = gateway.settings().codec;
    let outbound = &conn.inner.outbound;
    if !codec.reads(&frame) {
        let reason = match codec {
            Codec::Json => "this gateway reads text frames",
            Codec::MsgPack => "this gateway reads binary frames",
        };
        outbound.request_close(1003, reason.to_owned());
        return None;
    }
    let head = match envelope::parse(codec, &gateway.event_field, &frame) {
        Ok(head) => head,
        Err(bad) => {
            let error = CallError::new(ErrorKind::BadRequest, bad.reason);
            let _ = outbound.push(envelope::error(codec, bad.id.as_ref(), &error));
            return None;
        }
    };
    if head.event == "cancel" {
        match &head.id {
            Some(id) => flight.cancel(id),
            None => {
                let error = CallError::new(ErrorKind::BadRequest, "a `cancel` names the message it cancels by its `id`");
                let _ = outbound.push(envelope::error(codec, None, &error));
            }
        }
        return None;
    }
    let event = gateway.events.get(&head.event).map(|event| (event.mounted.clone(), Arc::clone(&event.call)));
    if !flight.begin() {
        return Some(frame);
    }
    tasks.spawn(message(conn.clone(), Arc::clone(flight), event, head));
    None
}

/// A hand-written gateway's messages, handed to `on_message` one at a time in order. A panic in
/// `on_message` is logged and closes the connection with 1011.
fn raw_inbox(
    tasks: &mut TaskSet,
    conn: Connection,
    instance: Instance,
    on_message: crate::gateway::MessageFn,
    flight: Arc<Flight>,
) -> mpsc::UnboundedSender<Frame> {
    let (sender, mut receiver) = mpsc::unbounded::<Frame>();
    tasks.spawn(async move {
        while let Some(frame) = receiver.next().await {
            let handled = AssertUnwindSafe(on_message(Arc::clone(&instance), conn.clone(), frame)).catch_unwind().await;
            if handled.is_err() {
                tracing::error!(gateway = %conn.inner.gateway.path, "a hand-written gateway's on_message panicked; the connection closes with 1011");
                conn.inner.outbound.request_close(1011, "internal error".to_owned());
            }
            flight.handled();
            flight.finished();
        }
    });
    sender
}

/// One message's execution: opened in its handler's module with the connection's inputs seeded,
/// `dispatch` with the handler's call, or `recover(None, ..)` with `Unimplemented` for an event
/// nothing handles, then the answer written.
///
/// A message without an `id` is fire-and-forget: success writes no ack and a failure writes the
/// `error` envelope without an id. A handler that returns nothing answers a message with an id
/// `{"id","complete":true}`.
async fn message(conn: Connection, flight: Arc<Flight>, event: Option<(MountedHandler<Ws>, HandlerFn)>, head: Head) {
    let _finished = Finished(Arc::clone(&flight));
    let mut handling = Handling(Some(Arc::clone(&flight)));
    let gateway = Arc::clone(&conn.inner.gateway);
    let codec = gateway.settings().codec;
    let outbound = Arc::clone(&conn.inner.outbound);
    let Head { event: name, id, data } = head;
    let module: ModuleRef = match &event {
        Some((mounted, _)) => mounted.module().clone(),
        None => gateway.connect.module().clone(),
    };
    let Ok(exec) = Execution::open(&module, ExecOptions::new()) else {
        let error = CallError::new(ErrorKind::Unavailable, "the server is shutting down");
        let _ = outbound.push(envelope::error(codec, id.as_ref(), &error));
        return;
    };
    conn.seed(&exec);
    let handle = exec.handle();
    let call = flight.register(id.clone(), handle.clone());
    let cx = WsCx { inner: Arc::new(CxInner { exec: handle.clone(), conn: conn.clone(), event: name.clone(), id: id.clone(), data }) };
    let handler_name = event.as_ref().map(|(mounted, _)| format!("{}::{}", gateway.controller, mounted.name()));
    let call_span = span::call(<Ws as Transport>::KEY, &name, handler_name.as_deref());
    call_span.record(span::WS_EVENT, name.as_str());
    let outcome = match &event {
        Some((mounted, call)) => {
            let call = Arc::clone(call);
            ulo::dispatch(mounted, &handle, &cx, move |cx| call(cx)).instrument(call_span.clone()).await
        }
        None => {
            let error = CallError::new(ErrorKind::Unimplemented, format!("no handler for event `{name}`"))
                .with_source(NoHandler { event: name.clone() });
            ulo::recover::<Ws>(None, &handle, &cx, Box::new(error)).instrument(call_span.clone()).await
        }
    };
    handling.done();
    match outcome {
        Err(err) => {
            let _ = outbound.push(envelope::error(codec, id.as_ref(), &CallError::from_boxed(err)));
        }
        Ok(Reply::None) => {
            if id.is_some() {
                let _ = outbound.push(envelope::complete(codec, id.as_ref()));
            }
        }
        Ok(Reply::One(frame)) => {
            if id.is_some() {
                let message = match envelope::data(codec, id.as_ref(), &frame) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::error!(%error, event = %name, "a handler's reply frame does not match its gateway's codec");
                        envelope::error(codec, id.as_ref(), &CallError::new(ErrorKind::Internal, "internal error"))
                    }
                };
                let _ = outbound.push(message);
            }
        }
        Ok(Reply::Many(stream)) => {
            // Tracked here, where it is written as the reply: a stream an interceptor discarded
            // never reaches this arm and reports nothing.
            let stream = Tracked::new(stream, handle.clone());
            let handler = event.as_ref().map(|(mounted, _)| mounted);
            pump(stream, handler, &handle, &cx, id.as_ref(), &outbound, codec).instrument(call_span).await;
        }
    }
    flight.unregister(call);
}

/// A streamed answer, each item written `{"id","data"}` as the client takes them, ended with
/// `{"id","complete":true}`. An `Err` item runs `dispatch_late` and is written as the `error`
/// envelope, which ends the stream; an error handler's `EndStream` ends it with `complete`. A
/// `cancel` for the message, or the connection ending, cuts it off.
async fn pump(
    mut stream: Tracked<BoxStream<'static, Result<Frame, BoxError>>>,
    handler: Option<&MountedHandler<Ws>>,
    exec: &ExecutionRef,
    cx: &WsCx,
    id: Option<&MessageId>,
    outbound: &Outbound,
    codec: Codec,
) {
    loop {
        // The cancellation is polled before the stream, so a cancelled message writes no further
        // item.
        let next = match future::select(pin!(exec.cancelled()), stream.next()).await {
            Either::Left(_) => return,
            Either::Right((item, _)) => item,
        };
        let last = match next {
            Some(Ok(frame)) => match envelope::data(codec, id, &frame) {
                Ok(message) => {
                    if outbound.push_wait(message).await.is_err() {
                        return;
                    }
                    continue;
                }
                Err(error) => {
                    tracing::error!(%error, "a streamed reply frame does not match its gateway's codec");
                    envelope::error(codec, id, &CallError::new(ErrorKind::Internal, "internal error"))
                }
            },
            Some(Err(err)) => late(err, handler, exec, cx, id, codec).await,
            None => envelope::complete(codec, id),
        };
        let _ = outbound.push_wait(last).await;
        return;
    }
}

async fn late(
    err: BoxError,
    handler: Option<&MountedHandler<Ws>>,
    exec: &ExecutionRef,
    cx: &WsCx,
    id: Option<&MessageId>,
    codec: Codec,
) -> Message {
    let original = CallError::from_boxed(err);
    let Some(handler) = handler else {
        return envelope::error(codec, id, &original);
    };
    let summary = original.summary();
    match ulo::dispatch_late(handler, exec, cx, Box::new(original)).await {
        LateOutcome::Render(reshaped) => envelope::error(codec, id, &CallError::from_boxed(reshaped)),
        LateOutcome::End => envelope::complete(codec, id),
        _ => {
            tracing::warn!(
                "an error handler answered `Ok` to a streamed item's error; the stream ends with the original error, \
                 since a late answer cannot replace it"
            );
            envelope::error(codec, id, &summary)
        }
    }
}

/// `on_disconnect` for a connection that connected, as a terminal execution without guards: a
/// plain execution while the app serves, one opened with the drain's token once it drains. Its
/// failures and panics are logged.
async fn disconnect(conn: &Connection, why: DisconnectReason, accept: &Accept) {
    let gateway = &accept.gateway;
    let Some(hook) = gateway.handler.disconnect.clone() else { return };
    let module = gateway.connect.module();
    let exec = match Execution::open(module, ExecOptions::new()) {
        Ok(exec) => exec,
        Err(_) => {
            let Some(token) = accept.tracker.token().await else { return };
            match Execution::open_terminal(&token, module, ExecOptions::new()) {
                Ok(exec) => exec,
                Err(_) => {
                    tracing::debug!(gateway = %gateway.path, "on_disconnect skipped: the drain had ended");
                    return;
                }
            }
        }
    };
    conn.seed(&exec);
    match AssertUnwindSafe(hook(exec.handle(), conn.clone(), why)).catch_unwind().await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, gateway = %gateway.path, "on_disconnect failed"),
        Err(_) => tracing::error!(gateway = %gateway.path, "on_disconnect panicked"),
    }
}
