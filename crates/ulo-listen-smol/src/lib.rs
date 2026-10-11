//! The smol listener for `ulo`'s hyper servers (transports DESIGN §3.7): `ulo-hyper-serve`'s
//! [`Listener`] over `async-net`'s sockets, with `futures-rustls` for TLS and `ulo-hyper-serve`'s
//! `FuturesIo` for hyper's I/O traits. No tokio runtime is involved: the sockets are `async-io`'s,
//! whose reactor runs on a thread of its own, and every task the server spawns goes through the
//! app's runtime, `ulo_smol::Smol` on a smol app.
//!
//! ```ignore
//! let app = App::builder(AppModule).runtime(ulo_smol::Smol::new(executor)).wire()?.connect().await?
//!     .bind(ulo_http_hyper::ServerOn::<ulo_listen_smol::SmolListener>::new("0.0.0.0:8080"))
//!     .listen().await?;
//! ```
//!
//! It adopts the sockets `ulo-net` bound or inherited, so endpoints, port 0 and socket activation
//! behave as on every other listener. hyper itself depends on tokio with its `sync` feature, so
//! tokio's synchronisation primitives are in this crate's tree through `ulo-hyper-serve`; nothing
//! else of tokio is.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_net::{TcpListener, TcpStream};
use futures_io::{AsyncRead, AsyncWrite};
use futures_rustls::TlsAcceptor;
use futures_rustls::server::TlsStream;
use ulo_hyper_serve::{FuturesIo, Incoming, Listener, tls_info};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

/// One socket adopted into `async-io`'s reactor, with its TLS acceptor when the server has TLS.
pub struct SmolListener {
    tcp: TcpListener,
    tls: Option<TlsAcceptor>,
}

impl Listener for SmolListener {
    type Io = FuturesIo<SmolStream>;

    fn adopt(listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        let tcp = TcpListener::try_from(listener.into_std())?;
        Ok(SmolListener { tcp, tls: tls.map(TlsAcceptor::from) })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let (stream, peer) = self.tcp.accept().await?;
        let local = stream.local_addr().ok();
        let Some(acceptor) = self.tls.clone() else {
            return Ok(Incoming::plain(FuturesIo::new(SmolStream(Inner::Plain(stream))), peer, local));
        };
        Ok(Incoming::handshaking(peer, local, async move {
            let stream = acceptor.accept(stream).await?;
            let info = tls_info(stream.get_ref().1);
            Ok((FuturesIo::new(SmolStream(Inner::Tls(Box::new(stream)))), info))
        }))
    }
}

/// An accepted connection on `futures-io`'s traits, plain or after its TLS handshake.
pub struct SmolStream(Inner);

enum Inner {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl SmolStream {
    /// Whether the connection completed a TLS handshake.
    pub fn is_tls(&self) -> bool {
        matches!(self.0, Inner::Tls(_))
    }
}

impl AsyncRead for SmolStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for SmolStream {
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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_flush(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Inner::Plain(stream) => Pin::new(stream).poll_close(cx),
            Inner::Tls(stream) => Pin::new(&mut **stream).poll_close(cx),
        }
    }
}
