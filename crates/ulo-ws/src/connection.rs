//! One connection: the read loop, the outbound queue, the limits and close codes, keep-alive on the
//! app's `Timer`, the per-message execution, and `on_disconnect` as a terminal execution
//! (transports DESIGN §4.1, §4.2). The handshake and the connection phase are here too, shared by
//! the hand-off on the HTTP server's port and the standalone server.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::{Future, pending};
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures_core::stream::BoxStream;
use futures_util::{FutureExt, SinkExt, StreamExt};
use http::header::{CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE, WWW_AUTHENTICATE};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::Instrument;
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, Closed, DrainToken, ExecOptions, Execution, ExecutionRef, LateOutcome,
    ModuleRef, MountedHandler, Timer, Transport,
};
use ulo_transport::{CallError, ErrorKind, Tracked, span};

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
pub(crate) struct Outbound {
    state: Mutex<OutState>,
    /// Wakes the read loop to write.
    wake: Notify,
    /// Wakes the stream pumps waiting for room.
    space: Notify,
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
            wake: Notify::new(),
            space: Notify::new(),
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
                    self.wake.notify_one();
                    return Err(Gone);
                }
            }
        }
        state.queue.push_back(message);
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    /// Queues `message` once the queue has room. A streamed answer is written at the pace the
    /// client reads it rather than tripping the overflow policy, which governs what the gateway
    /// cannot hold back: broadcasts and `Connection::send`.
    pub(crate) async fn push_wait(&self, message: Message) -> Result<(), Gone> {
        let mut message = Some(message);
        loop {
            let mut room = pin!(self.space.notified());
            room.as_mut().enable();
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
                    self.wake.notify_one();
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
        self.wake.notify_one();
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
        self.space.notify_waiters();
        (queue, close)
    }

    fn shut(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.queue.clear();
        drop(state);
        self.space.notify_waiters();
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
    phase: watch::Sender<Phase>,
    token: watch::Sender<Option<DrainToken>>,
    tasks: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    next: AtomicU64,
    live: watch::Sender<usize>,
}

impl Tracker {
    pub(crate) fn new() -> Arc<Tracker> {
        Arc::new(Tracker {
            phase: watch::channel(Phase::Serving).0,
            token: watch::channel(None).0,
            tasks: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            live: watch::channel(0).0,
        })
    }

    pub(crate) fn is_draining(&self) -> bool {
        *self.phase.borrow() == Phase::Draining
    }

    fn phase(&self) -> watch::Receiver<Phase> {
        self.phase.subscribe()
    }

    /// Runs `fut` on its own task until it ends or [`close`](Self::close) aborts it.
    pub(crate) fn spawn(self: &Arc<Self>, fut: impl Future<Output = ()> + Send + 'static) {
        let key = self.next.fetch_add(1, Ordering::Relaxed);
        self.live.send_modify(|live| *live += 1);
        let guard = TaskGuard { tracker: Arc::clone(self), key };
        // Spawned under the lock, so a task that ends at once removes its entry only after the
        // entry is in.
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        let task = tokio::spawn(async move {
            let _guard = guard;
            fut.await;
        });
        tasks.insert(key, task.abort_handle());
    }

    /// Raises the drain: idle connections close with 1001 at once, busy ones stop reading,
    /// finish their messages and then close. Returns once every connection has ended.
    pub(crate) async fn drain(&self, token: DrainToken) {
        self.token.send_replace(Some(token));
        self.phase.send_replace(Phase::Draining);
        let _ = self.live.subscribe().wait_for(|live| *live == 0).await;
    }

    /// Aborts every connection left and returns once their tasks have ended.
    pub(crate) async fn close(&self) {
        let handles: Vec<tokio::task::AbortHandle> =
            self.tasks.lock().unwrap_or_else(PoisonError::into_inner).values().cloned().collect();
        for handle in handles {
            handle.abort();
        }
        let _ = self.live.subscribe().wait_for(|live| *live == 0).await;
    }

    /// The drain's token, once the drain has begun.
    async fn token(&self) -> Option<DrainToken> {
        let mut token = self.token.subscribe();
        let ready = token.wait_for(Option::is_some).await.ok()?;
        ready.clone()
    }
}

struct TaskGuard {
    tracker: Arc<Tracker>,
    key: u64,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.tracker.tasks.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.key);
        self.tracker.live.send_modify(|live| *live = live.saturating_sub(1));
    }
}

/// What a connection on one gateway needs from the server that accepted it.
#[derive(Clone)]
pub(crate) struct Accept {
    pub(crate) gateway: Arc<GatewayRuntime>,
    pub(crate) hub: Arc<Hub>,
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) tracker: Arc<Tracker>,
}

/// The answer to an upgrade request, which each server writes as its own response type.
pub(crate) enum Answer {
    /// The 101, its headers.
    Switch(Vec<(HeaderName, HeaderValue)>),
    /// A refusal before the upgrade.
    Refuse { status: StatusCode, reason: String, headers: Vec<(HeaderName, HeaderValue)> },
}

impl Answer {
    fn refuse(status: StatusCode, reason: impl Into<String>) -> Answer {
        Answer::Refuse { status, reason: reason.into(), headers: Vec::new() }
    }
}

/// Answers one upgrade request on `accept`'s gateway: the RFC 6455 handshake checks, the
/// subprotocol negotiation, the connection phase before the 101 under `refuse = handshake`, and a
/// task, run by `accept.tracker`, that awaits `upgrade` once the 101 is written and runs the
/// connection.
pub(crate) async fn answer<Io, U>(accept: Accept, head: Parts, peer: Option<SocketAddr>, upgrade: U) -> Answer
where
    U: Future<Output = Result<Io, BoxError>> + Send + 'static,
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handshake = match check_handshake(&head, accept.gateway.settings()) {
        Ok(handshake) => handshake,
        Err(refusal) => return refusal,
    };
    if accept.tracker.is_draining() {
        return Answer::refuse(StatusCode::SERVICE_UNAVAILABLE, "the server is shutting down");
    }
    let head = Arc::new(head);
    let protocol: Option<Arc<str>> = handshake.protocol.as_deref().map(Arc::from);
    let admitted = match accept.gateway.settings().refuse {
        Refuse::Close => None,
        Refuse::Handshake => match connect(&accept, Arc::clone(&head), peer, protocol.clone()).await {
            Connected::Admitted(admitted) => Some(admitted),
            Connected::Refused(refused) => {
                let mut headers = Vec::new();
                if refused.status() == StatusCode::UNAUTHORIZED {
                    headers.push((WWW_AUTHENTICATE, HeaderValue::from_static("Bearer")));
                }
                return Answer::Refuse { status: refused.status(), reason: refused.reason().to_owned(), headers };
            }
        },
    };
    let tracker = Arc::clone(&accept.tracker);
    tracker.spawn(run(accept, head, peer, protocol, upgrade, admitted));
    Answer::Switch(handshake.headers)
}

/// The connection after its 101: the slot under `max_connections`, the connection phase unless
/// the handshake ran it, the read loop, then `on_disconnect` for a connection that connected.
async fn run<Io, U>(
    accept: Accept,
    head: Arc<Parts>,
    peer: Option<SocketAddr>,
    protocol: Option<Arc<str>>,
    upgrade: U,
    admitted: Option<Admitted>,
) where
    U: Future<Output = Result<Io, BoxError>> + Send + 'static,
    Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = match upgrade.await {
        Ok(io) => io,
        Err(error) => {
            tracing::debug!(%error, peer = ?peer, "a WebSocket upgrade did not complete");
            if let Some(admitted) = admitted {
                accept.hub.unregister(admitted.conn.id());
            }
            return;
        }
    };
    let limits = &accept.gateway.limits;
    let config = WebSocketConfig::default()
        .max_message_size(Some(limits.message_limit))
        .max_frame_size(Some(limits.frame_limit));
    let mut ws = WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await;
    let Some(slot) = Slot::acquire(&accept.gateway) else {
        if let Some(admitted) = admitted {
            accept.hub.unregister(admitted.conn.id());
        }
        refuse(&mut ws, &accept, 1013, "too many connections").await;
        return;
    };
    let admitted = match admitted {
        Some(admitted) => admitted,
        None => match connect(&accept, head, peer, protocol).await {
            Connected::Admitted(admitted) => admitted,
            Connected::Refused(refused) => {
                refuse(&mut ws, &accept, refused.close_code(), refused.reason()).await;
                return;
            }
        },
    };
    let why = serve(ws, &admitted, &accept).await;
    disconnect(&admitted.conn, why, &accept).await;
    accept.hub.unregister(admitted.conn.id());
    drop(slot);
}

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

struct Handshake {
    headers: Vec<(HeaderName, HeaderValue)>,
    protocol: Option<String>,
}

/// RFC 6455 §4.2.1's checks on the client's handshake, and the 101's headers: the accept key and
/// the first of the gateway's subprotocols the client offered. No extension is negotiated, so a
/// client offering `permessage-deflate` is answered without `Sec-WebSocket-Extensions`
/// (§9.1).
fn check_handshake(head: &Parts, settings: &GatewaySettings) -> Result<Handshake, Answer> {
    if head.method != Method::GET {
        return Err(Answer::Refuse {
            status: StatusCode::METHOD_NOT_ALLOWED,
            reason: "a WebSocket handshake is a GET".to_owned(),
            headers: vec![(http::header::ALLOW, HeaderValue::from_static("GET"))],
        });
    }
    if head.version < Version::HTTP_11 {
        return Err(Answer::refuse(StatusCode::BAD_REQUEST, "a WebSocket handshake is HTTP/1.1"));
    }
    if !has_token(&head.headers, &CONNECTION, "upgrade") || !has_token(&head.headers, &UPGRADE, "websocket") {
        return Err(Answer::refuse(StatusCode::BAD_REQUEST, "a WebSocket handshake carries `Connection: Upgrade` and `Upgrade: websocket`"));
    }
    if head.headers.get(SEC_WEBSOCKET_VERSION).map(HeaderValue::as_bytes) != Some(b"13".as_slice()) {
        return Err(Answer::Refuse {
            status: StatusCode::UPGRADE_REQUIRED,
            reason: "the WebSocket version is 13".to_owned(),
            headers: vec![(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"))],
        });
    }
    let Some(key) = head.headers.get(SEC_WEBSOCKET_KEY).filter(|key| !key.is_empty()) else {
        return Err(Answer::refuse(StatusCode::BAD_REQUEST, "a WebSocket handshake carries `Sec-WebSocket-Key`"));
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
        Err(_) => return Err(Answer::refuse(StatusCode::BAD_REQUEST, "`Sec-WebSocket-Key` is malformed")),
    }
    if let Some(value) = protocol.as_deref().and_then(|name| HeaderValue::from_str(name).ok()) {
        headers.push((SEC_WEBSOCKET_PROTOCOL, value));
    }
    Ok(Handshake { headers, protocol })
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

/// A connection the connection phase admitted, with a hand-written gateway's instance.
struct Admitted {
    conn: Connection,
    instance: Option<Instance>,
}

enum Connected {
    Admitted(Admitted),
    Refused(ConnectRefused),
}

/// The connection phase: one execution in the gateway's module with the connection's inputs
/// seeded, the session built in it before anything else runs, then `dispatch` of the connect
/// handler, its connect guards and its `OnConnect`. An error no error handler claims refuses
/// the connection by its kind.
async fn connect(accept: &Accept, head: Arc<Parts>, peer: Option<SocketAddr>, protocol: Option<Arc<str>>) -> Connected {
    let gateway = &accept.gateway;
    let connect = &gateway.connect;
    let Ok(exec) = Execution::open(connect.module(), ExecOptions::new()) else {
        return Connected::Refused(ConnectRefused { close_code: 1001, reason: "server shutting down".to_owned(), kind: None });
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
            timer: Arc::clone(&accept.timer),
        }),
    };
    accept.hub.register(id, Arc::clone(gateway), outbound);
    let handle = exec.handle();
    let cx = ConnectCx { inner: Arc::new(ConnectInner { exec: handle.clone(), conn: conn.clone() }) };
    let handler_name = format!("{}::connect", gateway.controller);
    let call_span = span::call(<WsConnect as Transport>::KEY, "ws.connect", Some(&handler_name));
    call_span.record(span::URL_PATH, &*gateway.path);
    let admit = Arc::clone(&gateway.handler.admit);
    let outcome = ulo::dispatch(connect, &handle, &cx, move |cx| admit(cx)).instrument(call_span).await;
    let reply = outcome.unwrap_or_else(|err| ConnectReply::Refused(ConnectRefused::of_error(err)));
    let refused = match reply {
        ConnectReply::Admitted => match &gateway.handler.messages {
            Messages::Envelope => return Connected::Admitted(Admitted { conn, instance: None }),
            Messages::Raw { instance, .. } => match instance(handle).await {
                Ok(instance) => return Connected::Admitted(Admitted { conn, instance: Some(instance) }),
                Err(error) => {
                    tracing::error!(%error, gateway = %gateway.path, "a hand-written gateway could not be resolved for a connection");
                    ConnectRefused::kind(ErrorKind::Internal, "internal error")
                }
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
    let mut deadline = accept.gateway.limits.pong_timeout.map(|after| accept.timer.sleep(after));
    loop {
        tokio::select! {
            next = ws.next() => match next {
                None | Some(Err(_)) => return,
                Some(Ok(Message::Close(_))) => {
                    let _ = ws.flush().await;
                }
                Some(Ok(_)) => {}
            },
            () = sleeping(&mut deadline) => return,
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

/// Resolves when `sleep` does; pending forever when there is none.
async fn sleeping(sleep: &mut Option<BoxFuture<'static, ()>>) {
    match sleep {
        Some(sleep) => sleep.await,
        None => pending().await,
    }
}

/// A connection's messages in flight, shared with their tasks.
#[derive(Default)]
struct Flight {
    /// Messages a handler is still answering, or a hand-written gateway's messages queued or in
    /// `on_message`: over `max_inflight`, the loop stops reading.
    handling: AtomicUsize,
    /// Messages not yet finished, a streamed answer still writing included: the drain waits for
    /// them.
    busy: AtomicUsize,
    progress: Notify,
    calls: Mutex<HashMap<u64, (Option<MessageId>, ExecutionRef)>>,
    next: AtomicU64,
}

impl Flight {
    fn begin(&self) {
        self.handling.fetch_add(1, Ordering::AcqRel);
        self.busy.fetch_add(1, Ordering::AcqRel);
    }

    fn handled(&self) {
        self.handling.fetch_sub(1, Ordering::AcqRel);
        self.progress.notify_one();
    }

    fn finished(&self) {
        self.busy.fetch_sub(1, Ordering::AcqRel);
        self.progress.notify_one();
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
    let flight = Arc::new(Flight::default());
    let mut tasks: JoinSet<()> = JoinSet::new();
    let inbox = match (&gateway.handler.messages, &admitted.instance) {
        (Messages::Raw { on_message, .. }, Some(instance)) => {
            Some(raw_inbox(&mut tasks, conn.clone(), Arc::clone(instance), Arc::clone(on_message), Arc::clone(&flight)))
        }
        _ => None,
    };
    let mut phase = accept.tracker.phase();
    let mut draining = *phase.borrow() == Phase::Draining;
    let mut closing: Option<DisconnectReason> = None;
    let mut close_deadline: Option<BoxFuture<'static, ()>> = None;
    let mut ping = limits.ping_interval.map(|every| accept.timer.sleep(every));
    let mut pong: Option<BoxFuture<'static, ()>> = None;

    let why = loop {
        let (queued, close) = outbound.take();
        if write(&mut ws, queued).await.is_err() {
            break closing.unwrap_or(DisconnectReason::Lost);
        }
        let close = match close {
            Some(close) if closing.is_none() => Some(close),
            _ if draining && closing.is_none() && flight.idle() => Some((1001, "server shutting down".to_owned())),
            _ => None,
        };
        if let Some((code, reason)) = close {
            let why = if code == 1001 && draining { DisconnectReason::Drain } else { DisconnectReason::ServerClose { code } };
            outbound.shut();
            if send_close(&mut ws, code, &reason).await.is_err() {
                break why;
            }
            closing = Some(why);
            close_deadline = limits.pong_timeout.map(|after| accept.timer.sleep(after));
        }
        let reading = closing.is_some() || (!draining && flight.handling() < limits.max_inflight);
        tokio::select! {
            next = ws.next(), if reading => match next {
                None => break closing.unwrap_or(DisconnectReason::Lost),
                Some(Ok(Message::Text(text))) if closing.is_none() => {
                    let frame = Frame::Text(text.as_str().to_owned());
                    received(frame, conn, &flight, &mut tasks, inbox.as_ref());
                }
                Some(Ok(Message::Binary(bytes))) if closing.is_none() => {
                    received(Frame::Binary(bytes), conn, &flight, &mut tasks, inbox.as_ref());
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
                        close_deadline = limits.pong_timeout.map(|after| accept.timer.sleep(after));
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
            () = outbound.wake.notified() => {}
            () = flight.progress.notified() => {}
            changed = phase.changed(), if !draining => {
                draining = changed.is_err() || *phase.borrow() == Phase::Draining;
            }
            () = sleeping(&mut ping), if ping.is_some() && closing.is_none() => {
                if ws.send(Message::Ping(Bytes::new())).await.is_err() {
                    break DisconnectReason::Lost;
                }
                ping = limits.ping_interval.map(|every| accept.timer.sleep(every));
                if pong.is_none() {
                    pong = limits.pong_timeout.map(|after| accept.timer.sleep(after));
                }
            }
            () = sleeping(&mut pong), if pong.is_some() => break DisconnectReason::Lost,
            () = sleeping(&mut close_deadline), if close_deadline.is_some() => {
                break closing.clone().unwrap_or(DisconnectReason::Lost);
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    };
    outbound.shut();
    flight.disconnected();
    drop(inbox);
    tasks.shutdown().await;
    why
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
/// A frame of the kind the gateway's codec does not read closes the connection with 1003.
fn received(frame: Frame, conn: &Connection, flight: &Arc<Flight>, tasks: &mut JoinSet<()>, inbox: Option<&mpsc::UnboundedSender<Frame>>) {
    if let Some(inbox) = inbox {
        flight.begin();
        if inbox.send(frame).is_err() {
            flight.handled();
            flight.finished();
        }
        return;
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
        return;
    }
    let head = match envelope::parse(codec, &gateway.event_field, &frame) {
        Ok(head) => head,
        Err(bad) => {
            let error = CallError::new(ErrorKind::BadRequest, bad.reason);
            let _ = outbound.push(envelope::error(codec, bad.id.as_ref(), &error));
            return;
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
        return;
    }
    let event = gateway.events.get(&head.event).map(|event| (event.mounted.clone(), Arc::clone(&event.call)));
    flight.begin();
    tasks.spawn(message(conn.clone(), Arc::clone(flight), event, head));
}

/// A hand-written gateway's messages, handed to `on_message` one at a time in order. A panic in
/// `on_message` is logged and closes the connection with 1011.
fn raw_inbox(
    tasks: &mut JoinSet<()>,
    conn: Connection,
    instance: Instance,
    on_message: crate::gateway::MessageFn,
    flight: Arc<Flight>,
) -> mpsc::UnboundedSender<Frame> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Frame>();
    tasks.spawn(async move {
        while let Some(frame) = receiver.recv().await {
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
        let next = tokio::select! {
            biased;
            () = exec.cancelled() => return,
            item = stream.next() => item,
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
