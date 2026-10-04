//! The sockets the loop accepts on, and a connection's I/O, plain or TLS.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
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

/// An accepted connection's I/O, plain or after its TLS handshake, as tokio's I/O traits. Wrap it
/// in `hyper_util::rt::TokioIo` for hyper's connection builders.
pub struct Io(pub(crate) Inner);

pub(crate) enum Inner {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Io {
    /// Whether the connection completed a TLS handshake.
    pub fn is_tls(&self) -> bool {
        matches!(self.0, Inner::Tls(_))
    }
}

impl AsyncRead for Io {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match &self.0 {
            Inner::Plain(stream) => stream.is_write_vectored(),
            Inner::Tls(stream) => (**stream).is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_shutdown(cx),
        }
    }
}
