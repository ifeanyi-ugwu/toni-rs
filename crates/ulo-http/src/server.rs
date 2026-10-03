use std::borrow::Cow;
use std::time::Duration;

use ulo::{BoundAddr, BoxError, DrainToken, Mounted};
use ulo_net::{EndpointSpec, Tls};

use crate::backend::{Backend, HttpConfig};
use crate::service::AppService;
use crate::transport::Http;

/// The HTTP server over backend `B`, the core's `Server` for [`Http`]: handed to `app.bind(..)`,
/// it carries addresses, TLS and limits; everything a route needs it reads from the mounted
/// handlers and module metadata.
///
/// ```ignore
/// app.bind(ulo_http_axum::Server::new("0.0.0.0:8080").tls(Tls::from_pem_files("cert.pem", "key.pem")))
/// ```
///
/// `prepare` builds the route table and the pre-dispatch stage, parses every endpoint, loads TLS,
/// checks the inherited sockets and the backend's limits, and reports every failure at once;
/// `bind` binds every endpoint, all-or-nothing, and hands the listeners to the backend.
pub struct Server<B: Backend> {
    pub(crate) endpoints: Vec<EndpointSpec>,
    pub(crate) tls: Option<Tls>,
    pub(crate) config: HttpConfig,
    pub(crate) backend: B,
    /// Set by `prepare`, read by `bind`.
    pub(crate) prepared: Option<Prepared>,
    pub(crate) bound: Vec<BoundAddr>,
}

/// What `prepare` built for `bind`.
pub(crate) struct Prepared {
    pub(crate) endpoints: Vec<ulo_net::Endpoint>,
    pub(crate) tls: Option<ulo_net::TlsAcceptor>,
    pub(crate) service: AppService,
}

impl<B: Backend + Default> Server<B> {
    /// A server on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Server::with_backend(endpoint, B::default())
    }
}

impl<B: Backend> Server<B> {
    pub fn with_backend(endpoint: impl Into<EndpointSpec>, backend: B) -> Self {
        Server { endpoints: vec![endpoint.into()], tls: None, config: HttpConfig::default(), backend, prepared: None, bound: Vec::new() }
    }

    /// One more endpoint the same server listens on.
    pub fn endpoint(mut self, endpoint: impl Into<EndpointSpec>) -> Self {
        self.endpoints.push(endpoint.into());
        self
    }

    /// TLS on every endpoint, ALPN `h2` and `http/1.1`, loaded in `prepare`.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// The body limit of a route without `#[meta(BodyLimit(..))]`.
    pub fn body_limit(mut self, bytes: u64) -> Self {
        self.config.body_limit = bytes;
        self
    }

    pub fn max_inflight(mut self, requests: usize) -> Self {
        self.config.max_inflight = Some(requests);
        self
    }

    /// HTTP/2 `SETTINGS_MAX_CONCURRENT_STREAMS` per connection.
    pub fn max_concurrent_streams(mut self, streams: u32) -> Self {
        self.config.max_concurrent_streams = Some(streams);
        self
    }

    pub fn shed_retry_after(mut self, after: Duration) -> Self {
        self.config.shed_retry_after = after;
        self
    }

    /// The `WWW-Authenticate` challenge of a 401 whose error names none; `Bearer` (RFC 6750) unset.
    pub fn challenge(mut self, challenge: impl Into<Cow<'static, str>>) -> Self {
        self.config.challenge = challenge.into();
        self
    }

    /// Accept HTTP/2 without TLS, refused in `prepare` on a backend whose limits forbid it.
    pub fn h2c(mut self, enabled: bool) -> Self {
        self.config.h2c = enabled;
        self
    }
}

impl<B: Backend> ulo::Server for Server<B> {
    type Transport = Http;

    async fn prepare(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!("router, stage and upgrades from the handlers and module metadata; endpoints resolved; TLS loaded; inherited sockets and `B::limits()` checked; every failure in one error")
    }

    async fn bind(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!("`ulo_net::bind_all` over the prepared endpoints, `B::bind`, record `bound`")
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.backend.serve().await
    }

    async fn drain(&self, token: DrainToken) {
        let _ = token;
        self.backend.drain().await
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.backend.close().await
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.bound.clone()
    }
}
