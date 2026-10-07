use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture};
use ulo_net::{Endpoint, EndpointSpec};
use ulo_rpc::link::{Inbound, UNARY_ONLY};
use ulo_rpc::{Ack, Capabilities, Codec, Delivery, DeliveryMode, Frame, FrameTooLarge, Link, Outbound, Pattern, ReplyPath, ReplyTo};

/// The largest UDP payload over IPv4: 65,535 less the IP and UDP headers. One frame is one
/// datagram, envelope included.
const MAX_DATAGRAM: u64 = 65_507;

/// Deliveries queued for the server; a full queue holds the receiving loop back, and the socket's
/// own buffer then drops what it cannot hold, as UDP does.
const DELIVERY_QUEUE: usize = 1024;

/// The UDP link: a server binds its endpoint, a client sends to it.
///
/// A server maps each sender's call ids to ids of its own, so two senders never share one, and
/// writes the sender's id back on its reply. No datagram is retried: a lost request or reply is
/// the caller's `Timeout`.
///
/// A client holds no connection, so `close` ends what stands for one: each socket the client side
/// bound stops receiving and is released, its reply lane ends, and the calls waiting on it fail
/// `Unavailable`. A call made afterwards binds a new socket.
pub struct Udp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) codec: Codec,
    /// Set by `prepare`.
    pub(crate) prepared: Option<SocketAddr>,
    pub(crate) state: Arc<State>,
}

pub(crate) struct State {
    bound: Mutex<Vec<BoundAddr>>,
    receiving: Mutex<Option<JoinHandle<()>>>,
    next_id: AtomicU64,
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
            prepared: None,
            state: Arc::new(State {
                bound: Mutex::new(Vec::new()),
                receiving: Mutex::new(None),
                next_id: AtomicU64::new(1),
                client_epoch: watch::Sender::new(0),
            }),
        }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
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

impl Link for Udp {
    const NAME: &'static str = "udp";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .binary(self.codec.binary())
            .max_frame(Some(MAX_DATAGRAM))
            .ordering(ulo_rpc::Ordering::Unordered)
            .shapes(UNARY_ONLY)
            .miss_signal(true)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
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
        let socket = Arc::new(UdpSocket::bind(addr).await?);
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", socket.local_addr()?)];
        let (deliveries, inbound) = mpsc::channel(DELIVERY_QUEUE);
        let receiving = tokio::spawn(receive(socket, self.codec, Arc::clone(&self.state), deliveries));
        *lock(&self.state.receiving) = Some(receiving);
        let inbound = futures_util::stream::unfold(inbound, |mut inbound| async move {
            let delivery = inbound.recv().await?;
            Some((delivery, inbound))
        });
        Ok(Box::pin(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let addr = self.address()?;
        let local: SocketAddr = if addr.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(addr).await?;
        // The reply lane holds the socket; sends reach it through a `Weak`, so the socket is
        // released once the lane ends, at `close` or when the OS reports the server unreachable.
        let socket = Arc::new(socket);
        let codec = self.codec;
        let sending = Arc::downgrade(&socket);
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let socket = Weak::clone(&sending);
            Box::pin(async move {
                let bytes = encode(codec, &frame)?;
                let socket = socket.upgrade().ok_or("the UDP link's client socket is closed")?;
                socket.send(&bytes).await?;
                Ok(())
            })
        });
        let buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
        let epoch = self.state.client_epoch.subscribe();
        let replies = futures_util::stream::unfold((socket, buffer, epoch), move |(socket, mut buffer, epoch)| async move {
            loop {
                let received = tokio::select! {
                    received = socket.recv(&mut buffer) => received,
                    () = closed(epoch.clone()) => return None,
                };
                // An error here is the OS reporting the server unreachable: the reply lane ends,
                // and the next call binds a new socket.
                let len = match received {
                    Ok(len) => len,
                    Err(error) => {
                        tracing::debug!(%error, "the UDP link's reply lane ended");
                        return None;
                    }
                };
                match codec.decode_frame(&buffer[..len]) {
                    Ok(frame) => return Some((frame, (socket, buffer, epoch))),
                    Err(error) => tracing::debug!(%error, "a reply datagram did not decode and was dropped"),
                }
            }
        });
        Ok(Outbound { send, replies: Box::pin(replies) })
    }

    /// UDP has no signal for a sender: the server answers each new call `unavailable` itself, and
    /// the socket keeps receiving so a `cancel` still reaches a call in flight.
    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        self.state.client_epoch.send_modify(|epoch| *epoch += 1);
        let receiving = lock(&self.state.receiving).take();
        if let Some(receiving) = receiving {
            receiving.abort();
            let _ = receiving.await;
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

/// Every datagram the socket receives, as deliveries, until the link closes. A datagram that does
/// not decode carries no id to answer and is dropped.
async fn receive(socket: Arc<UdpSocket>, codec: Codec, state: Arc<State>, deliveries: mpsc::Sender<Delivery>) {
    let ids = Arc::new(Mutex::new(Ids::default()));
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    loop {
        let (len, peer) = match socket.recv_from(&mut buffer).await {
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
        let Some(frame) = inward(frame, peer, &ids, &state.next_id) else { continue };
        let reply = reply_path(Arc::clone(&socket), peer, codec, Arc::clone(&ids));
        if deliveries.send(Delivery { frame, reply: Some(reply), ack: Ack::none() }).await.is_err() {
            return;
        }
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

/// The calls in flight: each sender's id and the server's, both ways.
#[derive(Default)]
struct Ids {
    by_caller: HashMap<(SocketAddr, u64), u64>,
    by_server: HashMap<u64, (SocketAddr, u64)>,
}

impl Ids {
    fn open(&mut self, peer: SocketAddr, caller: u64, next: &AtomicU64) -> u64 {
        let server = next.fetch_add(1, Ordering::Relaxed);
        if let Some(stale) = self.by_caller.insert((peer, caller), server) {
            self.by_server.remove(&stale);
        }
        self.by_server.insert(server, (peer, caller));
        server
    }

    fn server(&self, peer: SocketAddr, caller: u64) -> Option<u64> {
        self.by_caller.get(&(peer, caller)).copied()
    }

    fn cancel(&mut self, peer: SocketAddr, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&(peer, caller))?;
        self.by_server.remove(&server);
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<u64> {
        self.by_server.get(&server).map(|(_, caller)| *caller)
    }

    fn finish(&mut self, server: u64) {
        if let Some(caller) = self.by_server.remove(&server) {
            self.by_caller.remove(&caller);
        }
    }
}

/// Replies to one sender, under its own id; the mapping is forgotten once a reply ends the call.
fn reply_path(socket: Arc<UdpSocket>, peer: SocketAddr, codec: Codec, ids: Arc<Mutex<Ids>>) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let socket = Arc::clone(&socket);
        let ids = Arc::clone(&ids);
        Box::pin(async move {
            let Some(server) = frame.id() else {
                return Err("a reply frame carries no id".into());
            };
            let Some(caller) = lock(&ids).caller(server) else {
                return Err("the call is no longer in flight".into());
            };
            let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
            let bytes = encode(codec, &with_id(frame, caller))?;
            socket.send_to(&bytes, peer).await?;
            if ends {
                lock(&ids).finish(server);
            }
            Ok(())
        })
    })
    .peer(peer)
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
