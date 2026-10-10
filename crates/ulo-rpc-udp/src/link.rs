use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture};
use ulo_tokio::Tokio;
use ulo_net::{Endpoint, EndpointSpec};
use ulo_rpc::link::{Inbound, UNARY_ONLY};
use ulo_rpc::{
    Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::{Details, ErrorKind};

/// The largest UDP payload over IPv4: 65,535 less the IP and UDP headers. One frame is one
/// datagram, envelope included.
const MAX_DATAGRAM: u64 = 65_507;

/// Deliveries queued for the server; a full queue holds the receiving loop back, and the socket's
/// own buffer then drops what it cannot hold, as UDP does.
const DELIVERY_QUEUE: usize = 1024;

/// Reply frames read ahead of the client; a full queue holds the reply socket's reader back.
const REPLY_QUEUE: usize = 64;

/// Datagrams queued for one socket's writer; a full queue makes the next send wait.
const WRITE_QUEUE: usize = 64;

/// The UDP link: a server binds its endpoint, a client sends to it.
///
/// A server maps each sender's call ids to ids of its own, so two senders never share one, and
/// writes the sender's id back on its reply. No datagram is retried: a lost request or reply is
/// the caller's `Timeout`. The inbound stream ends once the draining server holds no call, and the
/// socket answers a request arriving afterwards `err` of kind `unavailable` itself, as the server
/// answers one during the drain, until `close`.
///
/// A client holds no connection, so `close` ends what stands for one: each socket the client side
/// bound stops receiving and is released, its reply lane ends, and the calls waiting on it fail
/// `Unavailable`. A call made afterwards binds a new socket.
///
/// Each socket has one writer task, fed in order by every send on it, so the datagrams one
/// socket sends leave it in the order they were sent.
///
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
pub struct Udp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// Set by `prepare`.
    pub(crate) prepared: Option<SocketAddr>,
    pub(crate) state: Arc<State>,
}

pub(crate) struct State {
    bound: Mutex<Vec<BoundAddr>>,
    /// The server's socket, read once more by `close` after the receive task has stopped.
    socket: Mutex<Option<Arc<UdpSocket>>>,
    /// The server socket's writer queue, which `close` refuses what the socket still holds through.
    writes: Mutex<Option<mpsc::Sender<Datagram>>>,
    receiving: Mutex<Option<JoinHandle<()>>>,
    next_id: AtomicU64,
    /// Raised by `drain`.
    draining: watch::Sender<bool>,
    /// Calls in flight: ids mapped and not yet forgotten.
    held: watch::Sender<usize>,
    /// Advanced by `close`; a client socket's reply lane ends when it changes.
    client_epoch: watch::Sender<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Udp {
    /// The link on `endpoint`: text parsed in `prepare`, or a `SocketAddr`. An inherited endpoint
    /// is refused in `prepare`, since the activation adopts listening TCP sockets alone.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Udp {
            endpoint: endpoint.into(),
            codec: Codec::Json,
            runtime: Tokio::try_current(),
            prepared: None,
            state: Arc::new(State {
                bound: Mutex::new(Vec::new()),
                socket: Mutex::new(None),
                writes: Mutex::new(None),
                receiving: Mutex::new(None),
                next_id: AtomicU64::new(1),
                draining: watch::Sender::new(false),
                held: watch::Sender::new(0),
                client_epoch: watch::Sender::new(0),
            }),
        }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The tokio runtime the link's sockets and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }

    /// The endpoint as written on both sides: `resolve` would turn an address `ulo dev --listen`
    /// holds as a TCP socket into that socket, which a datagram socket cannot be.
    fn address(&self) -> Result<SocketAddr, BoxError> {
        match self.endpoint.resolve_as_written()? {
            Endpoint::Addr(addr) => Ok(addr),
            endpoint => Err(format!(
                "{endpoint}: the UDP link takes an address; the activation adopts listening TCP sockets alone"
            )
            .into()),
        }
    }
}

const NO_RUNTIME: &str = "the UDP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Udp {
    const NAME: &'static str = "udp";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .binary(self.codec.binary())
            .max_frame(Some(MAX_DATAGRAM))
            .ordering(ulo_rpc::Ordering::Unordered)
            .shapes(UNARY_ONLY)
            .miss_signal(true)
            // `close` answers what its socket holds; a datagram arriving after the socket closes
            // reaches no one. A connected client socket may learn of it through an ICMP port
            // unreachable, which is often filtered, so the link does not rely on it.
            .confirms_drain(false)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        self.usable()?;
        self.prepared = Some(self.address()?);
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        let Some(addr) = self.prepared else {
            return Err("the UDP link listens once `prepare` has resolved its endpoint".into());
        };
        if lock(&self.state.receiving).is_some() {
            return Err("the UDP link is already listening".into());
        }
        let runtime = self.runtime()?;
        let socket = Arc::new(runtime.run(UdpSocket::bind(addr)).await??);
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", socket.local_addr()?)];
        *lock(&self.state.socket) = Some(Arc::clone(&socket));
        let (writes, queued) = mpsc::channel(WRITE_QUEUE);
        runtime.handle().spawn(write_datagrams(Arc::clone(&socket), queued, std::future::pending()));
        *lock(&self.state.writes) = Some(writes.clone());
        let (deliveries, inbound) = mpsc::channel(DELIVERY_QUEUE);
        let receiving = runtime.handle().spawn(receive(socket, self.codec, Arc::clone(&self.state), deliveries, writes));
        *lock(&self.state.receiving) = Some(receiving);
        let inbound = futures_util::stream::unfold(inbound, |mut inbound| async move {
            let delivery = inbound.recv().await?;
            Some((delivery, inbound))
        });
        Ok(Box::pin(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let addr = self.address()?;
        let runtime = self.runtime()?;
        let local: SocketAddr = if addr.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
        let socket = runtime
            .run(async move {
                let socket = UdpSocket::bind(local).await?;
                socket.connect(addr).await?;
                Ok::<_, io::Error>(socket)
            })
            .await??;
        // The reply lane holds the socket; the writer reaches it through a `Weak`, so the socket is
        // released once the lane ends, at `close` or when the OS reports the server unreachable.
        let socket = Arc::new(socket);
        let codec = self.codec;
        let epoch = self.state.client_epoch.subscribe();
        let (writes, queued) = mpsc::channel(WRITE_QUEUE);
        runtime.handle().spawn(write_datagrams(Arc::downgrade(&socket), queued, closed(epoch.clone())));
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let writes = writes.clone();
            Box::pin(async move {
                let bytes = encode(codec, &frame)?;
                write(&writes, bytes, None).await
            })
        });
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        runtime.handle().spawn(read_replies(socket, epoch, codec, frames));
        let replies = futures_util::stream::unfold(replies, |mut replies| async move {
            let frame = replies.recv().await?;
            Some((frame, replies))
        });
        Ok(Outbound { send, replies: Box::pin(replies) })
    }

    /// UDP has no signal for a sender: the server answers each new call `unavailable` itself, and
    /// the socket keeps receiving so a `cancel` still reaches a call in flight.
    async fn drain(&self) {
        self.state.draining.send_replace(true);
    }

    /// A server's `close` stops the receive task, then reads what the socket's buffer already
    /// holds and answers each request found there `err` of kind `unavailable`, so a datagram that
    /// arrived before the close is answered; one arriving after it is not.
    async fn close(&self) -> Result<(), BoxError> {
        self.state.client_epoch.send_modify(|epoch| *epoch += 1);
        let receiving = lock(&self.state.receiving).take();
        if let Some(receiving) = receiving {
            receiving.abort();
            let _ = receiving.await;
        }
        let socket = lock(&self.state.socket).take();
        let writes = lock(&self.state.writes).take();
        if let (Some(socket), Some(writes)) = (socket, writes) {
            let codec = self.codec;
            self.runtime()?.run(async move { final_read(&socket, codec, &writes).await }).await?;
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        lock(&self.state.bound).clone()
    }
}

/// Resolves once the client epoch moves past the one `epoch` was subscribed at, or the link is
/// gone.
async fn closed(mut epoch: watch::Receiver<u64>) {
    let _ = epoch.changed().await;
}

/// Reads the client socket's reply datagrams into `frames` until the link closes, the reply lane
/// is dropped, or the OS reports the server unreachable; the socket is released as this returns.
async fn read_replies(socket: Arc<UdpSocket>, epoch: watch::Receiver<u64>, codec: Codec, frames: mpsc::Sender<Frame>) {
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    loop {
        let received = tokio::select! {
            received = socket.recv(&mut buffer) => received,
            () = closed(epoch.clone()) => return,
            () = frames.closed() => return,
        };
        // An error here is the OS reporting the server unreachable: the reply lane ends, and the
        // next call binds a new socket.
        let len = match received {
            Ok(len) => len,
            Err(error) => {
                tracing::debug!(%error, "the UDP link's reply lane ended");
                return;
            }
        };
        match codec.decode_frame(&buffer[..len]) {
            Ok(frame) => {
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
            Err(error) => tracing::debug!(%error, "a reply datagram did not decode and was dropped"),
        }
    }
}

/// Every datagram the socket receives, as deliveries, until the link closes. A datagram that does
/// not decode carries no id to answer and is dropped. Once the link is draining and holds no call,
/// the inbound stream ends, and a request arriving afterwards is refused here: nothing tells a UDP
/// caller to stop sending, and the stream ending only at `close` would hold the drain to its
/// deadline. This loop alone sends deliveries and decides the end between datagrams, so a
/// `cancel` releasing the last call is queued before the stream ends.
async fn receive(socket: Arc<UdpSocket>, codec: Codec, state: Arc<State>, deliveries: mpsc::Sender<Delivery>, writes: mpsc::Sender<Datagram>) {
    let ids = Arc::new(Mutex::new(Ids::new(Arc::clone(&state))));
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    let mut deliveries = Some(deliveries);
    loop {
        let received = tokio::select! {
            biased;
            received = socket.recv_from(&mut buffer) => received,
            () = idle(&state), if deliveries.is_some() => {
                deliveries = None;
                continue;
            }
        };
        let (len, peer) = match received {
            Ok(received) => received,
            // A sender's ICMP refusal surfaces here on some platforms; the socket stays usable.
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused) => {
                tracing::debug!(%error, "the UDP link's socket reported a sender's refusal");
                continue;
            }
            // Any other failure ends the inbound stream, which before the drain stops the server
            // and starts the shutdown.
            Err(error) => {
                tracing::error!(%error, "the UDP link's socket failed");
                return;
            }
        };
        let frame = match codec.decode_frame(&buffer[..len]) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!(%error, %peer, "a datagram did not decode and was dropped");
                continue;
            }
        };
        let Some(sender) = deliveries.as_ref() else {
            refuse(&writes, codec, peer, &frame).await;
            continue;
        };
        let Some(frame) = inward(frame, peer, &ids, &state.next_id) else { continue };
        let reply = reply_path(writes.clone(), peer, codec, Arc::clone(&ids));
        if sender.send(Delivery { frame, reply: Some(reply), ack: Ack::none() }).await.is_err() {
            return;
        }
    }
}

/// Every datagram the socket's receive buffer holds, read without waiting until it is empty; each
/// request or streamed request is answered `err` of kind `unavailable`. The read goes through a
/// duplicate of the socket, not tokio's: tokio tries a read only once its reactor has seen the
/// socket readable, and a datagram that arrived since its last turn would be left unread.
async fn final_read(socket: &UdpSocket, codec: Codec, writes: &mpsc::Sender<Datagram>) {
    let duplicate = match socket2::SockRef::from(socket).try_clone().and_then(|duplicate| {
        duplicate.set_nonblocking(true)?;
        Ok(std::net::UdpSocket::from(duplicate))
    }) {
        Ok(duplicate) => duplicate,
        Err(error) => {
            tracing::warn!(%error, "the UDP link could not read its socket at close; requests still in its buffer go unanswered");
            return;
        }
    };
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    let mut refused = 0usize;
    loop {
        let (len, peer) = match duplicate.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused) => continue,
            Err(error) => {
                tracing::debug!(%error, "the UDP link's read at close failed");
                break;
            }
        };
        let Ok(frame) = codec.decode_frame(&buffer[..len]) else { continue };
        if matches!(frame, Frame::Req { .. } | Frame::Open { .. }) {
            refused += 1;
        }
        refuse(writes, codec, peer, &frame).await;
    }
    if refused > 0 {
        tracing::debug!(refused, "the UDP link answered requests left in its socket at close");
    }
}

/// Resolves once the link is draining and holds no call.
async fn idle(state: &State) {
    let mut draining = state.draining.subscribe();
    let mut held = state.held.subscribe();
    let _ = draining.wait_for(|draining| *draining).await;
    let _ = held.wait_for(|held| *held == 0).await;
}

/// Answers a request or streamed request the server will not take, under the sender's own id;
/// anything else names no call in flight and is dropped.
async fn refuse(writes: &mpsc::Sender<Datagram>, codec: Codec, peer: SocketAddr, frame: &Frame) {
    let (Frame::Req { id, .. } | Frame::Open { id, .. }) = *frame else { return };
    let error = ErrorBody::new(ErrorKind::Unavailable, "the server is shutting down", Details::new());
    let refused = match encode(codec, &Frame::Err { id, error }) {
        Ok(bytes) => write(writes, bytes, Some(peer)).await,
        Err(error) => Err(error),
    };
    if let Err(error) = refused {
        tracing::debug!(%error, %peer, "the UDP link could not refuse a request that arrived after the drain");
    }
}

/// A sender's frame with its id mapped to the server's; `None` for one the server takes no part
/// in, or that names no call in flight.
fn inward(frame: Frame, peer: SocketAddr, ids: &Mutex<Ids>, next: &AtomicU64) -> Option<Frame> {
    let mut ids = lock(ids);
    Some(match frame {
        Frame::Req { id, pattern, headers, data } => Frame::Req { id: ids.open(peer, id, next), pattern, headers, data },
        Frame::Open { id, pattern, headers } => Frame::Open { id: ids.open(peer, id, next), pattern, headers },
        Frame::Evt { pattern, headers, data } => Frame::Evt { pattern, headers, data },
        Frame::In { id, data } => Frame::In { id: ids.server(peer, id)?, data },
        Frame::InEnd { id } => Frame::InEnd { id: ids.server(peer, id)? },
        Frame::Cancel { id } => Frame::Cancel { id: ids.cancel(peer, id)? },
        _ => return None,
    })
}

/// The calls in flight: each sender's id and the server's, both ways, counted in `State::held`.
struct Ids {
    by_caller: HashMap<(SocketAddr, u64), u64>,
    by_server: HashMap<u64, (SocketAddr, u64)>,
    state: Arc<State>,
}

impl Ids {
    fn new(state: Arc<State>) -> Self {
        Ids { by_caller: HashMap::new(), by_server: HashMap::new(), state }
    }

    fn counted(&self) {
        self.state.held.send_replace(self.by_server.len());
    }

    fn open(&mut self, peer: SocketAddr, caller: u64, next: &AtomicU64) -> u64 {
        let server = next.fetch_add(1, Ordering::Relaxed);
        if let Some(stale) = self.by_caller.insert((peer, caller), server) {
            self.by_server.remove(&stale);
        }
        self.by_server.insert(server, (peer, caller));
        self.counted();
        server
    }

    fn server(&self, peer: SocketAddr, caller: u64) -> Option<u64> {
        self.by_caller.get(&(peer, caller)).copied()
    }

    fn cancel(&mut self, peer: SocketAddr, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&(peer, caller))?;
        self.by_server.remove(&server);
        self.counted();
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<u64> {
        self.by_server.get(&server).map(|(_, caller)| *caller)
    }

    fn finish(&mut self, server: u64) {
        if let Some(caller) = self.by_server.remove(&server) {
            self.by_caller.remove(&caller);
        }
        self.counted();
    }
}

/// Replies to one sender, under its own id, through the server socket's writer; the mapping is
/// forgotten once a reply ends the call.
fn reply_path(writes: mpsc::Sender<Datagram>, peer: SocketAddr, codec: Codec, ids: Arc<Mutex<Ids>>) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let writes = writes.clone();
        let ids = Arc::clone(&ids);
        Box::pin(async move {
            let Some(server) = frame.id() else {
                return Err(BoxError::from("a reply frame carries no id"));
            };
            let Some(caller) = lock(&ids).caller(server) else {
                return Err("the call is no longer in flight".into());
            };
            let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
            let bytes = encode(codec, &with_id(frame, caller))?;
            write(&writes, bytes, Some(peer)).await?;
            if ends {
                lock(&ids).finish(server);
            }
            Ok(())
        })
    })
    .peer(peer)
}

/// One datagram for a socket's writer: to `to` on the server's socket, to the connected address on
/// a client's, the writer's outcome answered on `answer`.
struct Datagram {
    bytes: Bytes,
    to: Option<SocketAddr>,
    answer: oneshot::Sender<Result<(), BoxError>>,
}

/// Queues `bytes` on a socket's writer and waits for it to be sent. The queue is taken in the
/// order sends are first polled, so one socket's datagrams leave it in that order.
async fn write(writes: &mpsc::Sender<Datagram>, bytes: Bytes, to: Option<SocketAddr>) -> Result<(), BoxError> {
    let (answer, answered) = oneshot::channel();
    writes.send(Datagram { bytes, to, answer }).await.map_err(|_| BoxError::from("the UDP link's socket is closed"))?;
    answered.await.map_err(|_| BoxError::from("the UDP link's socket is closed"))?
}

/// The socket a writer sends on: held by the server's writer, reached through a `Weak` by a
/// client's, whose reply lane owns the socket.
trait Sending: Send + 'static {
    fn socket(&self) -> Option<Arc<UdpSocket>>;
}

impl Sending for Arc<UdpSocket> {
    fn socket(&self) -> Option<Arc<UdpSocket>> {
        Some(Arc::clone(self))
    }
}

impl Sending for Weak<UdpSocket> {
    fn socket(&self) -> Option<Arc<UdpSocket>> {
        self.upgrade()
    }
}

/// Sends each queued datagram in turn until the queue's senders are gone or `stop` resolves.
async fn write_datagrams(socket: impl Sending, mut queued: mpsc::Receiver<Datagram>, stop: impl Future<Output = ()>) {
    let mut stop = std::pin::pin!(stop);
    loop {
        let Datagram { bytes, to, answer } = tokio::select! {
            datagram = queued.recv() => match datagram {
                Some(datagram) => datagram,
                None => return,
            },
            () = &mut stop => return,
        };
        let sent = match socket.socket() {
            Some(socket) => match to {
                Some(peer) => socket.send_to(&bytes, peer).await,
                None => socket.send(&bytes).await,
            }
            .map(drop)
            .map_err(BoxError::from),
            None => Err("the UDP link's client socket is closed".into()),
        };
        let _ = answer.send(sent);
    }
}

fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

/// One datagram's bytes, refused with `FrameTooLarge` over 65,507 before anything is sent.
fn encode(codec: Codec, frame: &Frame) -> Result<Bytes, BoxError> {
    let bytes = codec.encode_frame(frame)?;
    let size = bytes.len() as u64;
    if size > MAX_DATAGRAM {
        return Err(Box::new(FrameTooLarge { size, limit: MAX_DATAGRAM }));
    }
    Ok(bytes)
}
