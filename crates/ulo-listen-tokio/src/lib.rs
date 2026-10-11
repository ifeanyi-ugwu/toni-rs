//! The tokio listener for `ulo`'s hyper servers (transports DESIGN §3.7): `ulo-hyper-serve`'s
//! [`Listener`] over tokio's sockets, with `tokio-rustls` for TLS and hyper-util's `TokioIo` for
//! hyper's I/O traits. The listener the HTTP backend, the standalone WebSocket server and the gRPC
//! server use on tokio, and their default.
//!
//! ```ignore
//! // `ulo_http_hyper::Server` is the hyper backend on this listener.
//! let app = app.bind(ulo_http_hyper::Server::new("0.0.0.0:8080")).listen().await?;
//! ```
//!
//! It adopts the sockets `ulo-net` bound or inherited, so endpoints, port 0 and socket activation
//! behave as on every other listener. Adopting needs a tokio runtime current: an app whose
//! runtime is `ulo_tokio::Tokio` binds its servers inside one, and any other app's `bind` is
//! refused naming the listener.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use ulo_hyper_serve::{Incoming, Listener, tls_info};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

/// One socket adopted into tokio's reactor, with its TLS acceptor when the server has TLS.
pub struct TokioListener {
    tcp: TcpListener,
    tls: Option<TlsAcceptor>,
}

impl Listener for TokioListener {
    type Io = TokioIo<TokioStream>;

    fn adopt(listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(io::Error::other(format!(
                "the tokio listener cannot adopt {}: no tokio runtime is running; give the app `ulo_tokio::Tokio` as its \
                 runtime, or bind the server on the listener of the runtime the app runs on",
                listener.endpoint()
            )));
        }
        let tcp = TcpListener::from_std(listener.into_std())?;
        Ok(TokioListener { tcp, tls: tls.map(TlsAcceptor::from) })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let (stream, peer) = self.tcp.accept().await?;
        let local = stream.local_addr().ok();
        let Some(acceptor) = self.tls.clone() else {
            return Ok(Incoming::plain(TokioIo::new(TokioStream(Inner::Plain(stream))), peer, local));
        };
        Ok(Incoming::handshaking(peer, local, async move {
            let stream = acceptor.accept(stream).await?;
            let info = tls_info(stream.get_ref().1);
            Ok((TokioIo::new(TokioStream(Inner::Tls(Box::new(stream)))), info))
        }))
    }
}

/// An accepted connection on tokio's I/O traits, plain or after its TLS handshake.
pub struct TokioStream(Inner);

enum Inner {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl TokioStream {
    /// Whether the connection completed a TLS handshake.
    pub fn is_tls(&self) -> bool {
        matches!(self.0, Inner::Tls(_))
    }
}

impl AsyncRead for TokioStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TokioStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
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
