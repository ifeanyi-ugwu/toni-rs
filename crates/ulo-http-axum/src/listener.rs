//! A listener `axum::serve` accepts on, plain or TLS, through axum's `serve::Listener` trait.

use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// One bound endpoint, its TLS acceptor when the server has TLS. A failed handshake is logged and
/// the listener accepts the next connection.
pub(crate) struct Listener {
    pub(crate) tcp: TcpListener,
    pub(crate) tls: Option<TlsAcceptor>,
}

/// A connection, plain or after its TLS handshake.
pub(crate) enum Io {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl axum::serve::Listener for Listener {
    type Io = Io;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        todo!("accept; run the TLS handshake when configured, retrying on a failed handshake or a transient accept error")
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

impl tokio::io::AsyncRead for Io {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
            Io::Tls(stream) => std::pin::Pin::new(&mut **stream).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Io {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
            Io::Tls(stream) => std::pin::Pin::new(&mut **stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => std::pin::Pin::new(stream).poll_flush(cx),
            Io::Tls(stream) => std::pin::Pin::new(&mut **stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
            Io::Tls(stream) => std::pin::Pin::new(&mut **stream).poll_shutdown(cx),
        }
    }
}
