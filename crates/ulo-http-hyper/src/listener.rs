//! The sockets the backend accepts on, and a connection's I/O, plain or TLS.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use ulo_http::TlsInfo;

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
    tracing::error!(%error, "accepting an HTTP connection failed; retrying in one second");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// The TLS handshake on `stream`, with what it settled, bounded by `timeout` when there is one. A
/// failed or timed-out handshake is routine (scanners, clients that refuse the certificate), so it
/// is logged at `debug` with the peer and the connection dropped.
pub(crate) async fn handshake(
    acceptor: &TlsAcceptor,
    stream: TcpStream,
    peer: SocketAddr,
    timeout: Option<Duration>,
) -> Option<(Io, TlsInfo)> {
    let accepted = match timeout {
        Some(timeout) => tokio::time::timeout(timeout, acceptor.accept(stream)).await,
        None => Ok(acceptor.accept(stream).await),
    };
    match accepted {
        Ok(Ok(stream)) => {
            let (_, conn) = stream.get_ref();
            let mut info = TlsInfo::new();
            if let Some(alpn) = conn.alpn_protocol() {
                info = info.alpn(alpn.to_vec());
            }
            if let Some(name) = conn.server_name() {
                info = info.server_name(name);
            }
            Some((Io::Tls(Box::new(stream)), info))
        }
        Ok(Err(error)) => {
            tracing::debug!(%peer, %error, "TLS handshake failed");
            None
        }
        Err(_) => {
            tracing::debug!(%peer, ?timeout, "TLS handshake timed out");
            None
        }
    }
}

/// A connection, plain or after its TLS handshake.
pub(crate) enum Io {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Io {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Io::Tls(stream) => Pin::new(&mut **stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Io::Tls(stream) => Pin::new(&mut **stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Io::Tls(stream) => Pin::new(&mut **stream).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Io::Plain(stream) => stream.is_write_vectored(),
            Io::Tls(stream) => (**stream).is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Io::Tls(stream) => Pin::new(&mut **stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Io::Tls(stream) => Pin::new(&mut **stream).poll_shutdown(cx),
        }
    }
}
