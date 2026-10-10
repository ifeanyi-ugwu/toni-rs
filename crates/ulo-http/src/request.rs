use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::request::Parts;
use http::uri::PathAndQuery;
use http::{HeaderMap, Method, Uri};
use futures_io::{AsyncRead, AsyncWrite};
use ulo::{BoxError, BoxFuture};

use crate::body::HttpBody;

/// What a backend hands [`AppService::call`](crate::AppService::call) per request, and what a
/// [`Middleware`](crate::Middleware) receives: the `http` crate's request parts, the body, the
/// connection, and the upgrade future when the backend can hand one over.
pub struct Request {
    pub head: Parts,
    pub body: HttpBody,
    pub conn: ConnInfo,
    /// Resolves to the connection's I/O once the response with status 101 has been written.
    /// `None` when the request asks for no upgrade, and on every request to a backend or an
    /// embedding whose limits declare `upgrades: false`.
    pub upgrade: Option<OnUpgrade>,
}

impl Request {
    pub fn method(&self) -> &Method {
        &self.head.method
    }

    pub fn path(&self) -> &str {
        self.head.uri.path()
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    /// Rewrites the path, keeping the query. In an unscoped pre-dispatch entry it changes which
    /// route matches, since unscoped entries run before routing.
    ///
    /// A path without a leading `/` gets one, since an origin-form request target always starts
    /// with it (RFC 9112 §3.2.1). A path that is not a valid URI path fails and leaves the request
    /// unchanged.
    pub fn set_path(&mut self, path: &str) -> Result<(), http::Error> {
        let slash = if path.starts_with('/') { "" } else { "/" };
        let path_and_query = match self.head.uri.query() {
            Some(query) => format!("{slash}{path}?{query}"),
            None => format!("{slash}{path}"),
        };
        let mut parts = self.head.uri.clone().into_parts();
        parts.path_and_query = Some(PathAndQuery::try_from(path_and_query)?);
        self.head.uri = Uri::from_parts(parts)?;
        Ok(())
    }
}

/// The connection a request arrived on.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ConnInfo {
    pub peer: Option<SocketAddr>,
    pub local: Option<SocketAddr>,
    pub tls: Option<TlsInfo>,
    pub version: http::Version,
}

impl ConnInfo {
    pub fn new(version: http::Version) -> Self {
        ConnInfo { peer: None, local: None, tls: None, version }
    }

    pub fn peer(self, peer: SocketAddr) -> Self {
        ConnInfo { peer: Some(peer), ..self }
    }

    pub fn local(self, local: SocketAddr) -> Self {
        ConnInfo { local: Some(local), ..self }
    }

    pub fn tls(self, tls: TlsInfo) -> Self {
        ConnInfo { tls: Some(tls), ..self }
    }
}

/// What the TLS handshake settled.
#[non_exhaustive]
#[derive(Clone, Debug, Default)]
pub struct TlsInfo {
    pub alpn: Option<Vec<u8>>,
    pub server_name: Option<String>,
}

impl TlsInfo {
    pub fn new() -> Self {
        TlsInfo::default()
    }

    pub fn alpn(self, alpn: Vec<u8>) -> Self {
        TlsInfo { alpn: Some(alpn), ..self }
    }

    pub fn server_name(self, name: impl Into<String>) -> Self {
        TlsInfo { server_name: Some(name.into()), ..self }
    }
}

/// The backend's per-request upgrade future (transports DESIGN §3.5, §3.7).
pub struct OnUpgrade {
    inner: BoxFuture<'static, Result<Upgraded, BoxError>>,
}

impl OnUpgrade {
    pub fn new(fut: impl Future<Output = Result<Upgraded, BoxError>> + Send + 'static) -> Self {
        OnUpgrade { inner: Box::pin(fut) }
    }
}

impl Future for OnUpgrade {
    type Output = Result<Upgraded, BoxError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl fmt::Debug for OnUpgrade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OnUpgrade")
    }
}

/// An upgraded connection's I/O, whatever the backend's type, as `futures-io`'s `AsyncRead` and
/// `AsyncWrite`.
///
/// Built by [`from_futures`](Self::from_futures) from I/O implementing `futures-io`'s traits, or,
/// with the `tokio-io` feature, by `Upgraded::from_tokio` from I/O implementing tokio's. The
/// feature also implements tokio's traits on it, both directions through `tokio-util`'s `compat`
/// layer, so an upgrade handler written against tokio reads and writes it through tokio's traits,
/// whichever constructor built it.
pub struct Upgraded {
    io: Pin<Box<dyn Io>>,
}

trait Io: AsyncRead + AsyncWrite + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Send + 'static> Io for T {}

impl Upgraded {
    /// An upgraded connection whose I/O implements `futures-io`'s traits, as smol's does.
    pub fn from_futures(io: impl AsyncRead + AsyncWrite + Send + Unpin + 'static) -> Self {
        Upgraded { io: Box::pin(io) }
    }

    /// An upgraded connection whose I/O implements tokio's traits, as hyper's does through
    /// `TokioIo`.
    #[cfg(feature = "tokio-io")]
    pub fn from_tokio(io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static) -> Self {
        use tokio_util::compat::TokioAsyncReadCompatExt;
        Upgraded::from_futures(io.compat())
    }
}

impl AsyncRead for Upgraded {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.io.as_mut().poll_read(cx, buf)
    }
}

impl AsyncWrite for Upgraded {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.io.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.as_mut().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.as_mut().poll_close(cx)
    }
}

// Each call wraps the boxed I/O in a fresh `Compat`, which holds nothing across reads and writes
// besides a seek position these impls never use.
#[cfg(feature = "tokio-io")]
impl tokio::io::AsyncRead for Upgraded {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<io::Result<()>> {
        use tokio_util::compat::FuturesAsyncReadCompatExt;
        tokio::io::AsyncRead::poll_read(Pin::new(&mut self.io.as_mut().compat()), cx, buf)
    }
}

#[cfg(feature = "tokio-io")]
impl tokio::io::AsyncWrite for Upgraded {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        use tokio_util::compat::FuturesAsyncWriteCompatExt;
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut self.io.as_mut().compat_write()), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        use tokio_util::compat::FuturesAsyncWriteCompatExt;
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut self.io.as_mut().compat_write()), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        use tokio_util::compat::FuturesAsyncWriteCompatExt;
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut self.io.as_mut().compat_write()), cx)
    }
}
