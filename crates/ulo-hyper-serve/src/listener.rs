//! The sockets the loop accepts on, and a connection's I/O, plain or TLS.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// One bound endpoint, its TLS acceptor when the server has TLS.
pub(crate) struct Listener {
    pub(crate) tcp: TcpListener,
    pub(crate) tls: Option<TlsAcceptor>,
}

impl Listener {
    /// The next connection. The TLS handshake is the connection task's, so a slow one never holds
    /// up the accept loop.
    pub(crate) async fn accept(&self) -> (TcpStream, SocketAddr) {
        loop {
            match self.tcp.accept().await {
                Ok(accepted) => return accepted,
                Err(error) => backoff(error).await,
            }
        }
    }
}

/// A connection the peer abandoned between its arrival and the accept is skipped at once. Any
/// other error, running out of file descriptors above all, would fail again immediately, so the
/// loop waits a second first, as hyper's own accept loop did.
async fn backoff(error: io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(%error, "accepting a connection failed; retrying in one second");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// How many connections a [`Serve`](crate::Serve) has handed to its server that the server has
/// read from: each counted at its first read that yields bytes, after the TLS handshake where
/// there is one, and never counted down. Cheap to clone; every clone reads the same count.
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

/// An accepted connection's I/O, plain or after its TLS handshake, as tokio's I/O traits. Wrap it
/// in `hyper_util::rt::TokioIo` for hyper's connection builders.
pub struct Io {
    inner: Inner,
    /// Taken, and counted, at the first read that yields bytes.
    unread: Option<ReadCount>,
}

pub(crate) enum Inner {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Io {
    pub(crate) fn new(inner: Inner) -> Io {
        Io { inner, unread: None }
    }

    /// This connection counted into `reads` at its first read that yields bytes.
    pub(crate) fn counting(self, reads: ReadCount) -> Io {
        Io { unread: Some(reads), ..self }
    }

    /// Whether the connection completed a TLS handshake.
    pub fn is_tls(&self) -> bool {
        matches!(self.inner, Inner::Tls(_))
    }
}

impl AsyncRead for Io {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let io = self.get_mut();
        let before = buf.filled().len();
        let read = match &mut io.inner {
            Inner::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_read(cx, buf),
        };
        if matches!(read, Poll::Ready(Ok(()))) && buf.filled().len() > before && let Some(reads) = io.unread.take() {
            reads.0.fetch_add(1, Ordering::AcqRel);
        }
        read
    }
}

impl AsyncWrite for Io {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().inner {
            Inner::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().inner {
            Inner::Plain(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match &self.inner {
            Inner::Plain(stream) => stream.is_write_vectored(),
            Inner::Tls(stream) => (**stream).is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_shutdown(cx),
        }
    }
}
