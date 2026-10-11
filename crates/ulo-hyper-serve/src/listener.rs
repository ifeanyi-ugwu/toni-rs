//! The plug point a runtime crate implements, what it hands over per connection, and a
//! connection's I/O counted at its first read.

use std::future::Future;
use std::io;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use hyper::rt::{Read, ReadBuf, ReadBufCursor, Write};
use ulo::BoxFuture;
use ulo_http::TlsInfo;
use ulo_net::BoundListener;
use ulo_net::rustls::{ServerConfig, ServerConnection};

/// Where a [`Serve`](crate::Serve)'s connections come from: one bound socket adopted into a
/// runtime's reactor, with that runtime's TLS stack. `ulo-listen-tokio` implements it over tokio's
/// sockets and `tokio-rustls`, `ulo-listen-smol` over `async-net` and `futures-rustls`; which one a
/// server is built with is the runtime it serves on.
///
/// Everything else a server does is the accept loop's, the same on every runtime: one task per
/// connection spawned through the app's `Runtime`, the handshake timeout timed by its `Timer`,
/// the per-connection count, the drain and the close.
pub trait Listener: Sized + Send + Sync + 'static {
    /// One connection's I/O as hyper reads and writes it, after the TLS handshake where the
    /// listener has TLS.
    type Io: Read + Write + Send + Unpin + 'static;

    /// Adopts `listener`, a socket `ulo-net` bound or inherited, into the runtime, accepting TLS
    /// under `tls`, the configuration a server's `Tls::load` built, when there is one. Called from
    /// a server's `bind`, inside the app's runtime; fails when the runtime cannot register the
    /// socket, or when the listener's own runtime is not the one running.
    fn adopt(listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<Self>;

    /// The next connection. An error is the accept's own, which the loop backs off from; the TLS
    /// handshake is not done here but in [`Incoming`]'s handshake, on the connection's own task, so
    /// a slow one never holds up the accept loop.
    fn accept(&self) -> impl Future<Output = io::Result<Incoming<Self::Io>>> + Send;
}

/// One accepted connection as a [`Listener`] hands it over: its addresses, and the handshake that
/// yields its I/O, which [`Serve`](crate::Serve) bounds by the handshake timeout and abandons at
/// the drain.
pub struct Incoming<T> {
    pub(crate) peer: SocketAddr,
    pub(crate) local: Option<SocketAddr>,
    pub(crate) handshake: BoxFuture<'static, io::Result<(T, Option<TlsInfo>)>>,
}

impl<T: Send + 'static> Incoming<T> {
    /// A connection with no handshake: its I/O is ready as accepted.
    pub fn plain(io: T, peer: SocketAddr, local: Option<SocketAddr>) -> Incoming<T> {
        Incoming { peer, local, handshake: Box::pin(std::future::ready(Ok((io, None)))) }
    }

    /// A connection whose I/O `handshake` yields, with what the TLS handshake settled.
    pub fn handshaking<F>(peer: SocketAddr, local: Option<SocketAddr>, handshake: F) -> Incoming<T>
    where
        F: Future<Output = io::Result<(T, TlsInfo)>> + Send + 'static,
    {
        let handshake = async move { handshake.await.map(|(io, info)| (io, Some(info))) };
        Incoming { peer, local, handshake: Box::pin(handshake) }
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }
}

/// What a finished TLS handshake settled, as a request's `ConnInfo` reports it: the ALPN protocol
/// and the SNI name. A listener reads it off its TLS crate's session, `tokio-rustls`' and
/// `futures-rustls`' alike being rustls's `ServerConnection`.
pub fn tls_info(session: &ServerConnection) -> TlsInfo {
    let mut info = TlsInfo::new();
    if let Some(alpn) = session.alpn_protocol() {
        info = info.alpn(alpn.to_vec());
    }
    if let Some(name) = session.server_name() {
        info = info.server_name(name);
    }
    info
}

/// How many connections a [`Serve`](crate::Serve) has handed to its server that the server has
/// read from: each counted at its first read that yields bytes, after the TLS handshake where there
/// is one, and never counted down. Cheap to clone; every clone reads the same count.
///
/// A test reads it to know the server has begun a request before acting on it, the drain above
/// all. A listener closed at the drain resets a connection still in its backlog, and hyper's
/// graceful shutdown closes a connection it has read nothing from as idle (`hyper-1.11.1`,
/// `proto/h1/dispatch.rs:91-100`), so a connection sent half a request is shown in progress only
/// once the server has read some of it.
#[derive(Clone, Debug, Default)]
pub struct ReadCount(Arc<AtomicUsize>);

impl ReadCount {
    /// The connections read from so far.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

/// An accepted connection's I/O, as hyper's I/O traits, counted into the server's [`ReadCount`] at
/// its first read that yields bytes. Hand it to hyper's connection builders as it is.
pub struct Io<T> {
    inner: T,
    /// Taken, and counted, at the first read that yields bytes.
    unread: Option<ReadCount>,
}

impl<T> Io<T> {
    pub(crate) fn counting(inner: T, reads: ReadCount) -> Io<T> {
        Io { inner, unread: Some(reads) }
    }

    /// The listener's I/O, for a server that reads what its runtime's stream reports.
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
}

impl<T: Read + Unpin> Read for Io<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.unread.is_none() {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }
        // The cursor is moved into the read it is handed to, so it cannot report afterwards how
        // far it was filled; the unfilled memory goes to the inner read under a buffer of its own,
        // as hyper-util's `TokioIo` reads through tokio's, until the first read is counted.
        //
        // SAFETY: `as_mut` hands out the unfilled memory, of which this initializes nothing
        // itself; the inner read writes initialized bytes into it through `ReadBuf`, which reports
        // how many, and `advance` moves the cursor past exactly those.
        let filled = unsafe {
            let unfilled: &mut [MaybeUninit<u8>] = buf.as_mut();
            let mut probe = ReadBuf::uninit(unfilled);
            match Pin::new(&mut this.inner).poll_read(cx, probe.unfilled()) {
                Poll::Ready(Ok(())) => probe.filled().len(),
                other => return other,
            }
        };
        if filled > 0
            && let Some(reads) = this.unread.take()
        {
            reads.0.fetch_add(1, Ordering::AcqRel);
        }
        // SAFETY: the inner read initialized `filled` bytes at the start of the unfilled memory.
        unsafe { buf.advance(filled) };
        Poll::Ready(Ok(()))
    }
}

impl<T: Write + Unpin> Write for Io<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}
