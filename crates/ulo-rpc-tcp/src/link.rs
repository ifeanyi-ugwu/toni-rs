use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture};
use ulo_tokio::Tokio;
use ulo_net::{Activation, ActivationError, BoundListener, Endpoint, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::__private::ordered;
use ulo_transport::{Details, ErrorKind};

/// The largest frame unless `max_frame` sets one: twice HTTP's default body limit, room for a
/// payload HTTP accepts unset with its envelope and its JSON encoding's growth.
const DEFAULT_MAX_FRAME: u64 = 4 * 1024 * 1024;

/// How long the accept loop waits after the OS refuses an accept, out of descriptors or memory,
/// before it accepts again.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// Frames queued for one connection's writer ahead of the next send's wait: each send is queued at its call and waits until it is
/// among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// Deliveries queued for the server; a full queue holds each connection's reader back.
const DELIVERY_QUEUE: usize = 1024;

/// Reply frames read ahead of the client; a full queue holds the connection's reader back.
const REPLY_QUEUE: usize = 64;

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const CLOSED: u8 = 2;

/// The TCP link: a server binds its endpoint, a client connects to it.
///
/// A server maps each connection's call ids to ids of its own, so calls on two connections never
/// share one, and writes the caller's id back on every reply. A connection that closes cancels its
/// calls in flight `Disconnected`. At the drain the listener closes and every connection receives
/// `goaway`, one registered after the drain began included, while its calls in flight finish. The inbound stream ends once the draining server
/// holds no call, and a request arriving afterwards, on a connection the caller kept open, is
/// answered `err` of kind `unavailable` by the link, as the server answers one during the drain.
///
/// `close` also ends every connection the link's client side opened: its socket is shut and its
/// reply lane ends, so the calls waiting on it fail `Unavailable`. A call made afterwards connects
/// again.
///
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
pub struct Tcp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) tls: Option<Tls>,
    pub(crate) codec: Codec,
    pub(crate) max_frame: Option<u64>,
    pub(crate) runtime: Option<Tokio>,
    /// Set by `prepare`.
    pub(crate) prepared: Option<Prepared>,
    pub(crate) state: Arc<State>,
}

pub(crate) struct Prepared {
    endpoint: Endpoint,
    acceptor: Option<TlsAcceptor>,
}

/// The server side's running state, shared with its tasks.
pub(crate) struct State {
    bound: Mutex<Vec<BoundAddr>>,
    /// Each open connection's writer queue, for `goaway`.
    connections: Mutex<HashMap<u64, ordered::Sender<Bytes>>>,
    accept: Mutex<Option<JoinHandle<()>>>,
    phase: watch::Sender<u8>,
    next_connection: AtomicU64,
    next_id: AtomicU64,
    /// Calls in flight across every connection: ids mapped and not yet forgotten.
    held: watch::Sender<usize>,
    /// Advanced by `close`; a client connection ends when it changes.
    client_epoch: watch::Sender<u64>,
    /// The writer of each client connection, which `close` waits for once it has shut the socket.
    client_writers: Mutex<JoinSet<()>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. A
    /// server may listen on an inherited socket; a client connects to an address.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp {
            endpoint: endpoint.into(),
            tls: None,
            codec: Codec::Json,
            max_frame: None,
            runtime: Tokio::try_current(),
            prepared: None,
            state: Arc::new(State {
                bound: Mutex::new(Vec::new()),
                connections: Mutex::new(HashMap::new()),
                accept: Mutex::new(None),
                phase: watch::Sender::new(RUNNING),
                next_connection: AtomicU64::new(0),
                next_id: AtomicU64::new(1),
                held: watch::Sender::new(0),
                client_epoch: watch::Sender::new(0),
                client_writers: Mutex::new(JoinSet::new()),
            }),
        }
    }

    /// TLS on the server's endpoint, loaded in `prepare`, with no ALPN. The client side speaks
    /// plain TCP: a client link with it set refuses to connect.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The largest frame, in bytes, read or written; a larger length prefix closes the connection
    /// before the body is read. 4 MiB unset; a value above the 4-byte prefix's
    /// range is bounded by it. Zero is refused in `prepare`.
    pub fn max_frame(mut self, bytes: u64) -> Self {
        self.max_frame = Some(bytes);
        self
    }

    /// The tokio runtime the link's sockets and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn limit(&self) -> u64 {
        self.max_frame.unwrap_or(DEFAULT_MAX_FRAME).min(u64::from(u32::MAX))
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the TCP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Tcp {
    const NAME: &'static str = "tcp";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .binary(self.codec.binary())
            .max_frame(Some(self.limit()))
            .ordering(ulo_rpc::Ordering::PerConnection)
            .miss_signal(true)
            .ordered_control(true)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        let mut problems = Vec::new();
        if let Err(error) = self.usable() {
            problems.push(error.to_string());
        }
        let endpoint = match self.endpoint.resolve() {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                problems.push(error.to_string());
                None
            }
        };
        if let Some(Endpoint::Inherited(name)) = &endpoint {
            match Activation::get() {
                Ok(activation) if activation.contains(name) => {}
                Ok(_) => problems.push(ActivationError::Missing(name.clone()).to_string()),
                Err(error) => problems.push(error.to_string()),
            }
        }
        let acceptor = match &self.tls {
            Some(tls) => match tls.load(&[]) {
                Ok(config) => Some(TlsAcceptor::from(config)),
                Err(error) => {
                    problems.push(error.to_string());
                    None
                }
            },
            None => None,
        };
        if self.max_frame == Some(0) {
            problems.push("`.max_frame(0)` would refuse every frame".to_owned());
        }
        match (endpoint, problems.as_slice()) {
            (Some(endpoint), []) => {
                self.prepared = Some(Prepared { endpoint, acceptor });
                Ok(())
            }
            (_, [problem]) => Err(problem.clone().into()),
            (_, problems) => Err(format!("{} problems:\n  - {}", problems.len(), problems.join("\n  - ")).into()),
        }
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        let Some(prepared) = &self.prepared else {
            return Err("the TCP link listens once `prepare` has resolved its endpoint".into());
        };
        let runtime = self.runtime()?;
        let handle = runtime.handle();
        let mut accept = lock(&self.state.accept);
        if accept.is_some() {
            return Err("the TCP link is already listening".into());
        }
        let listener = BoundListener::bind(&prepared.endpoint)?;
        let local = listener.local_addr();
        let listener = {
            // Registered with the link's runtime, whose tasks accept on it.
            let _entered = handle.enter();
            TcpListener::from_std(listener.into_std())?
        };
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", local).tls(prepared.acceptor.is_some())];
        let (deliveries, received) = mpsc::channel(DELIVERY_QUEUE);
        let (forwarded, inbound) = mpsc::channel(DELIVERY_QUEUE);
        let server = Server {
            acceptor: prepared.acceptor.clone(),
            codec: self.codec,
            limit: self.limit(),
            state: Arc::clone(&self.state),
            handle: handle.clone(),
        };
        *accept = Some(handle.spawn(server.accept(listener, deliveries)));
        handle.spawn(forward(Arc::clone(&self.state), received, forwarded, handle.clone()));
        let inbound = futures_util::stream::unfold(inbound, |mut inbound| async move {
            let delivery = inbound.recv().await?;
            Some((delivery, inbound))
        });
        Ok(Box::pin(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        // As written: under `ulo dev`, `resolve` would turn the address of the socket the command
        // holds into that socket, which a client cannot connect through.
        let endpoint = self.endpoint.resolve_as_written()?;
        let Endpoint::Addr(addr) = endpoint else {
            return Err(format!("a TCP client connects to an address; {endpoint} is a socket a server listens on").into());
        };
        if self.tls.is_some() {
            return Err("the TCP link's client side speaks no TLS; `Tcp::tls` sets a server's certificate".into());
        }
        let runtime = self.runtime()?;
        let stream = runtime.run(TcpStream::connect(addr)).await??;
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        let epoch = self.state.client_epoch.subscribe();
        {
            let mut writers = lock(&self.state.client_writers);
            while writers.try_join_next().is_some() {}
            writers.spawn_on(write_frames(writer, queued, closed(epoch.clone())), runtime.handle());
        }
        let (codec, limit) = (self.codec, self.limit());
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        runtime.handle().spawn(read_replies(reader, epoch, codec, limit, frames));
        // Queued at the call, so frames go out in the order their sends were called.
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let queued = encode(codec, limit, &frame).map(|bytes| queue.send(bytes));
            Box::pin(async move { queued?.await.map_err(|_| BoxError::from("the TCP connection closed")) })
        });
        let replies = futures_util::stream::unfold(replies, |mut replies| async move {
            let frame = replies.recv().await?;
            Some((frame, replies))
        });
        Ok(Outbound { send, replies: Box::pin(replies) })
    }

    async fn drain(&self) {
        self.state.phase.send_if_modified(|phase| advance(phase, DRAINING));
        let Ok(goaway) = self.codec.encode_frame(&Frame::Goaway) else { return };
        let queues: Vec<ordered::Sender<Bytes>> = lock(&self.state.connections).values().cloned().collect();
        for queue in queues {
            // A connection whose queue is full is busy writing replies; its caller learns of the
            // drain from the `unavailable` its next call is answered with.
            if queue.try_send(goaway.clone()).is_err() {
                tracing::debug!("a TCP connection's write queue was full at the drain; it received no `goaway`");
            }
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.state.phase.send_if_modified(|phase| advance(phase, CLOSED));
        self.state.client_epoch.send_modify(|epoch| *epoch += 1);
        let accept = lock(&self.state.accept).take();
        if let Some(accept) = accept {
            let _ = accept.await;
        }
        // The connection tasks are gone; each writer ends, shutting its half of the socket, once
        // its queue's last sender drops, and this map holds one for every connection whose task
        // was aborted before it could remove its own.
        lock(&self.state.connections).clear();
        let mut writers = std::mem::take(&mut *lock(&self.state.client_writers));
        while writers.join_next().await.is_some() {}
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        lock(&self.state.bound).clone()
    }
}

fn advance(phase: &mut u8, to: u8) -> bool {
    if *phase < to {
        *phase = to;
        true
    } else {
        false
    }
}

/// Resolves once the client epoch moves past the one `epoch` was subscribed at, or the link is
/// gone.
async fn closed(mut epoch: watch::Receiver<u64>) {
    let _ = epoch.changed().await;
}

/// Reads one client connection's reply frames into `frames` until the connection ends, the link
/// closes, or the reply lane is dropped.
async fn read_replies(mut reader: OwnedReadHalf, epoch: watch::Receiver<u64>, codec: Codec, limit: u64, frames: mpsc::Sender<Frame>) {
    loop {
        let read = tokio::select! {
            read = read_frame(&mut reader, limit) => read,
            () = closed(epoch.clone()) => return,
            () = frames.closed() => return,
        };
        let bytes = match read {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(%error, "the TCP connection's reply lane ended");
                return;
            }
        };
        match codec.decode_frame(&bytes) {
            Ok(frame) => {
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "a reply frame did not decode; the TCP connection is dropped");
                return;
            }
        }
    }
}

/// Resolves once the phase reached `at`, or the link holding the sender is gone.
async fn reached(phase: &mut watch::Receiver<u8>, at: u8) {
    let _ = phase.wait_for(|phase| *phase >= at).await;
}

/// What every connection task of one server reads.
#[derive(Clone)]
struct Server {
    acceptor: Option<TlsAcceptor>,
    codec: Codec,
    limit: u64,
    state: Arc<State>,
    /// The link's runtime, which every connection's tasks are spawned on.
    handle: Handle,
}

impl Server {
    /// Accepts until the drain, then holds the connections until they end or the link closes. The
    /// inbound stream ends once this task and every connection task have dropped their senders.
    async fn accept(self, listener: TcpListener, deliveries: mpsc::Sender<Delivery>) {
        let mut phase = self.state.phase.subscribe();
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = reached(&mut phase, DRAINING) => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        connections.spawn_on(self.clone().connection(stream, peer, deliveries.clone()), &self.handle);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the TCP link could not accept a connection");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                },
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        drop(listener);
        drop(deliveries);
        loop {
            tokio::select! {
                biased;
                () = reached(&mut phase, CLOSED) => {
                    connections.shutdown().await;
                    return;
                }
                finished = connections.join_next() => if finished.is_none() {
                    return;
                },
            }
        }
    }

    async fn connection(self, stream: TcpStream, peer: SocketAddr, deliveries: mpsc::Sender<Delivery>) {
        let _ = stream.set_nodelay(true);
        match self.acceptor.clone() {
            Some(acceptor) => match acceptor.accept(stream).await {
                Ok(stream) => self.serve(stream, peer, deliveries).await,
                Err(error) => tracing::debug!(%error, %peer, "a TLS handshake on the TCP link failed"),
            },
            None => self.serve(stream, peer, deliveries).await,
        }
    }

    /// Reads one connection's frames until it closes, then cancels its calls in flight.
    async fn serve<IO>(self, io: IO, peer: SocketAddr, deliveries: mpsc::Sender<Delivery>)
    where
        IO: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (mut reader, writer) = tokio::io::split(io);
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        // The writer runs apart from this task: the server's calls hold reply paths into its queue
        // after the connection's reader has ended, and it ends when the last of them drops.
        self.handle.spawn(write_frames(writer, queued, std::future::pending()));
        let connection = self.state.next_connection.fetch_add(1, Ordering::Relaxed);
        lock(&self.state.connections).insert(connection, queue.clone());
        // A connection accepted before the drain and registered after it, its TLS handshake
        // finishing late, missed the drain's `goaway`: it sends its own. The drain advances the
        // phase before it reads this map, so one of the two always reaches it.
        if *self.state.phase.borrow() >= DRAINING
            && let Ok(goaway) = self.codec.encode_frame(&Frame::Goaway)
        {
            let _ = queue.try_send(goaway);
        }
        let ids = Arc::new(Mutex::new(Ids::new(Arc::clone(&self.state))));
        let path = reply_path(queue, self.codec, self.limit, Arc::clone(&ids), peer);
        loop {
            let bytes = match read_frame(&mut reader, self.limit).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(%error, %peer, "a TCP connection ended");
                    break;
                }
            };
            let frame = match self.codec.decode_frame(&bytes) {
                Ok(frame) => frame,
                Err(error) => {
                    // Without its id nothing can be answered: the connection is dropped.
                    tracing::warn!(%error, %peer, "a frame did not decode; the TCP connection is dropped");
                    break;
                }
            };
            let Some(frame) = self.inward(frame, &ids) else { continue };
            let cancels = matches!(frame, Frame::Cancel { .. });
            let delivery = Delivery { frame, reply: Some(path.clone()), ack: Ack::none() };
            let sent = deliveries.send(delivery).await;
            if cancels {
                self.state.held.send_modify(|held| *held = held.saturating_sub(1));
            }
            if sent.is_err() {
                break;
            }
        }
        lock(&self.state.connections).remove(&connection);
        let orphaned = lock(&ids).drain();
        let count = orphaned.len();
        for id in orphaned {
            let _ = deliveries.send(Delivery { frame: Frame::Cancel { id }, reply: None, ack: Ack::none() }).await;
        }
        // Released once their `cancel`s are queued, so the inbound stream cannot end before them.
        self.state.held.send_modify(|held| *held = held.saturating_sub(count));
    }

    /// A caller's frame with its id mapped to the server's; `None` for one the server takes no
    /// part in, or that names no call in flight.
    fn inward(&self, frame: Frame, ids: &Mutex<Ids>) -> Option<Frame> {
        let mut ids = lock(ids);
        Some(match frame {
            Frame::Req { id, pattern, headers, data } => {
                Frame::Req { id: ids.open(id, &self.state.next_id), pattern, headers, data }
            }
            Frame::Open { id, pattern, headers } => Frame::Open { id: ids.open(id, &self.state.next_id), pattern, headers },
            Frame::Evt { pattern, headers, data } => Frame::Evt { pattern, headers, data },
            Frame::In { id, data } => Frame::In { id: ids.server(id)?, data },
            Frame::InEnd { id } => Frame::InEnd { id: ids.server(id)? },
            Frame::Cancel { id } => Frame::Cancel { id: ids.cancel(id)? },
            _ => return None,
        })
    }
}

/// Hands the connections' deliveries to the server until the draining server holds no call, then
/// ends the inbound stream and answers what arrives afterwards itself: a request or streamed
/// request `err` of kind `unavailable`, which also forgets its id. Ending sooner would drop an
/// item or a `cancel` for a call still held; ending once every connection has closed would hold
/// the drain to its deadline while a client keeps an idle connection open. A call's count is
/// released only after its last delivery is queued here, and this reads the queue before it looks
/// at the count, so nothing queued before the count reached zero is answered by the link.
async fn forward(state: Arc<State>, mut received: mpsc::Receiver<Delivery>, forwarded: mpsc::Sender<Delivery>, handle: Handle) {
    let mut forwarded = Some(forwarded);
    loop {
        let Some(sender) = forwarded.as_ref() else {
            let Some(delivery) = received.recv().await else { return };
            refuse(delivery, &handle);
            continue;
        };
        tokio::select! {
            biased;
            delivery = received.recv() => match delivery {
                Some(delivery) => {
                    if let Err(unsent) = sender.send(delivery).await {
                        forwarded = None;
                        refuse(unsent.0, &handle);
                    }
                }
                None => return,
            },
            () = idle(&state) => forwarded = None,
        }
    }
}

/// Resolves once the link is draining and holds no call.
async fn idle(state: &State) {
    let mut phase = state.phase.subscribe();
    let mut held = state.held.subscribe();
    let _ = phase.wait_for(|phase| *phase >= DRAINING).await;
    let _ = held.wait_for(|held| *held == 0).await;
}

/// Answers a request or streamed request the server will not receive as the server answers one
/// during the drain; anything else names no call in flight and is dropped.
fn refuse(delivery: Delivery, handle: &Handle) {
    let (Frame::Req { id, .. } | Frame::Open { id, .. }) = delivery.frame else { return };
    let Some(reply) = delivery.reply else { return };
    let error = ErrorBody::new(ErrorKind::Unavailable, "the server is shutting down", Details::new());
    handle.spawn(async move {
        if let Err(error) = reply.send(Frame::Err { id, error }).await {
            tracing::debug!(%error, "the TCP link could not refuse a request that arrived after the drain");
        }
    });
}

/// One connection's calls in flight: the caller's id and the server's, both ways, counted in
/// `State::held`.
struct Ids {
    by_caller: HashMap<u64, u64>,
    by_server: HashMap<u64, u64>,
    state: Arc<State>,
}

impl Ids {
    fn new(state: Arc<State>) -> Self {
        Ids { by_caller: HashMap::new(), by_server: HashMap::new(), state }
    }

    /// Applies the change in calls in flight since `before`.
    fn counted(&self, before: usize) {
        let after = self.by_server.len();
        if after != before {
            self.state.held.send_modify(|held| *held = (*held + after).saturating_sub(before));
        }
    }

    fn open(&mut self, caller: u64, next: &AtomicU64) -> u64 {
        let before = self.by_server.len();
        let server = next.fetch_add(1, Ordering::Relaxed);
        if let Some(stale) = self.by_caller.insert(caller, server) {
            self.by_server.remove(&stale);
        }
        self.by_server.insert(server, caller);
        self.counted(before);
        server
    }

    fn server(&self, caller: u64) -> Option<u64> {
        self.by_caller.get(&caller).copied()
    }

    /// The caller cancelled: its call is no longer answered. Released once the `cancel` is
    /// queued for the server, by the connection's reader.
    fn cancel(&mut self, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&caller)?;
        self.by_server.remove(&server);
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<u64> {
        self.by_server.get(&server).copied()
    }

    fn finish(&mut self, server: u64) {
        let before = self.by_server.len();
        if let Some(caller) = self.by_server.remove(&server) {
            self.by_caller.remove(&caller);
        }
        self.counted(before);
    }

    /// Every server id still in flight, forgotten; the reader releases them once their `cancel`s
    /// are queued.
    fn drain(&mut self) -> Vec<u64> {
        self.by_caller.clear();
        self.by_server.drain().map(|(server, _)| server).collect()
    }
}

/// Replies to one connection: the server's id written back as the caller's, the mapping forgotten
/// once a reply ends the call.
fn reply_path(queue: ordered::Sender<Bytes>, codec: Codec, limit: u64, ids: Arc<Mutex<Ids>>, peer: SocketAddr) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        // Queued at the call, so replies go out in the order their sends were called.
        let queued = queue_reply(&queue, codec, limit, &ids, frame);
        Box::pin(async move { queued?.await.map_err(|_| BoxError::from("the TCP connection closed")) })
    })
    .peer(peer)
}

/// `frame` under the caller's id, queued now; the mapping is forgotten once a reply ends the
/// call.
fn queue_reply(queue: &ordered::Sender<Bytes>, codec: Codec, limit: u64, ids: &Mutex<Ids>, frame: Frame) -> Result<ordered::Room<Bytes>, BoxError> {
    let Some(server) = frame.id() else {
        return Err("a reply frame carries no id".into());
    };
    let Some(caller) = lock(ids).caller(server) else {
        return Err("the call is no longer in flight on this connection".into());
    };
    let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
    let bytes = encode(codec, limit, &with_id(frame, caller))?;
    let room = queue.send(bytes);
    if ends {
        lock(ids).finish(server);
    }
    Ok(room)
}

/// A reply frame under the caller's id.
fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

/// One frame's bytes, refused with `FrameTooLarge` over `limit` before anything is written.
fn encode(codec: Codec, limit: u64, frame: &Frame) -> Result<Bytes, BoxError> {
    let bytes = codec.encode_frame(frame)?;
    let size = bytes.len() as u64;
    if size > limit {
        return Err(Box::new(FrameTooLarge { size, limit }));
    }
    Ok(bytes)
}

/// One length-prefixed frame; `None` at a clean end between frames. A prefix over `limit` fails
/// before the body is read.
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u64::from(u32::from_be_bytes(prefix));
    if len > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes is over the limit of {limit} bytes")));
    }
    let mut body = vec![0; len as usize];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Writes each queued frame behind its length prefix until the queue's senders are gone or `stop`
/// resolves, then shuts the writer.
async fn write_frames<W: AsyncWrite + Unpin>(mut writer: W, mut queued: ordered::Receiver<Bytes>, stop: impl Future<Output = ()>) {
    let mut stop = std::pin::pin!(stop);
    loop {
        let bytes = tokio::select! {
            bytes = queued.recv() => match bytes {
                Some(bytes) => bytes,
                None => break,
            },
            () = &mut stop => break,
        };
        let prefix = (bytes.len() as u32).to_be_bytes();
        let written = async {
            writer.write_all(&prefix).await?;
            writer.write_all(&bytes).await?;
            writer.flush().await
        };
        if written.await.is_err() {
            return;
        }
    }
    let _ = writer.shutdown().await;
}
