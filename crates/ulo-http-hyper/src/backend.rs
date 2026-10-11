use std::convert::Infallible;
use std::future::poll_fn;
use std::marker::PhantomData;
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::server::conn::auto;
use ulo::BoxError;
use ulo_http::{AppService, Backend, BackendLimits, HttpConfig};
use ulo_hyper_serve::{Accepted, Listener, ReadCount, RuntimeExecutor, RuntimeTimer, Serve, ServeConfig};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_transport::Count;

use crate::convert;

/// The hyper backend on listener `L`, the runtime's sockets: `ulo_listen_tokio::TokioListener`
/// for [`Hyper`](crate::Hyper), `ulo_listen_smol::SmolListener` on smol. `Default`, so
/// `ulo_http::Server::<HyperOn<L>>::new(endpoint)` builds it.
///
/// `ulo-hyper-serve` accepts on every listener, one task per connection spawned through the app's
/// runtime, and hands each connection here after its TLS handshake, where hyper's connection
/// builders serve it with the server's settings applied, their executor and timer the app's
/// runtime and clock.
pub struct HyperOn<L: Listener> {
    read_count: ReadCount,
    /// Set by `bind`.
    pub(crate) bound: Option<Bound<L>>,
    _listener: PhantomData<fn() -> L>,
}

impl<L: Listener> Default for HyperOn<L> {
    fn default() -> Self {
        HyperOn { read_count: ReadCount::default(), bound: None, _listener: PhantomData }
    }
}

impl<L: Listener> HyperOn<L> {
    /// How many connections the backend has read from, through a clone taken before the server
    /// moves into the app: each counted at its first read, after its TLS handshake where the
    /// server has TLS.
    ///
    /// ```ignore
    /// let backend = ulo_http_hyper::Hyper::default();
    /// let read_count = backend.read_count();
    /// let app = app.bind(ulo_http_hyper::Server::with_backend("127.0.0.1:0", backend)).listen().await?;
    /// ```
    pub fn read_count(&self) -> ReadCount {
        self.read_count.clone()
    }
}

/// What `bind` prepared for `serve`.
pub(crate) struct Bound<L: Listener> {
    pub(crate) serve: Serve<L>,
    pub(crate) service: AppService,
    pub(crate) protocols: Arc<Protocols>,
}

/// How each connection is served, from the server's settings.
pub(crate) struct Protocols {
    /// HTTP/1.1, or HTTP/2 when the connection opens with its preface: a TLS connection, whose
    /// protocol ALPN settled, and a plain one when h2c is on.
    auto: auto::Builder<RuntimeExecutor>,
    /// HTTP/1.1 alone, for a plain connection when h2c is off, since the auto builder accepts the
    /// HTTP/2 preface on any connection it serves with upgrades.
    http1: http1::Builder,
    h2c: bool,
}

impl Protocols {
    /// `header_timeout` is hyper's HTTP/1.1 header-read timeout, set on both builders in every
    /// case, so the 30 seconds at `Bound::Default` is the server's value rather than hyper's and
    /// `Bound::Unbounded` clears it. The timer is what hyper reads that clock from, the app's;
    /// hyper panics on a header-read timeout configured without one. HTTP/2 has no head-read
    /// clock in hyper. HTTP/2's stream tasks are spawned on the app's runtime.
    ///
    /// `max_concurrent_streams` at `Count::Default` leaves hyper's own value; `Count::Unlimited`
    /// clears it, which sends no `SETTINGS_MAX_CONCURRENT_STREAMS`.
    fn new(cfg: &HttpConfig, svc: &AppService) -> Self {
        let header_timeout = cfg.header_timeout_after();
        let runtime = Arc::clone(svc.runtime());
        let timer = RuntimeTimer::new(Arc::clone(&runtime) as Arc<dyn ulo::Timer>);
        let mut auto = auto::Builder::new(RuntimeExecutor::new(runtime));
        auto.http1().timer(timer.clone()).header_read_timeout(header_timeout);
        match cfg.max_concurrent_streams {
            Count::Default => {}
            Count::Max(streams) => {
                auto.http2().max_concurrent_streams(streams);
            }
            Count::Unlimited => {
                auto.http2().max_concurrent_streams(None);
            }
        }
        let mut http1 = http1::Builder::new();
        http1.timer(timer).header_read_timeout(header_timeout);
        Protocols { auto, http1, h2c: cfg.h2c }
    }
}

impl<L: Listener> Backend for HyperOn<L> {
    const NAME: &'static str = "hyper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(
        &mut self,
        listeners: Vec<BoundListener>,
        tls: Option<Arc<ServerConfig>>,
        svc: AppService,
        cfg: &HttpConfig,
    ) -> Result<(), BoxError> {
        let config = ServeConfig { handshake_timeout: cfg.handshake_timeout_after(), read_count: self.read_count.clone() };
        let serve = Serve::<L>::new(listeners, tls, &config, Arc::clone(svc.runtime()))?;
        let protocols = Arc::new(Protocols::new(cfg, &svc));
        self.bound = Some(Bound { serve, service: svc, protocols });
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(bound) = &self.bound else {
            return Err("the hyper backend was asked to serve before it was bound".into());
        };
        let service = bound.service.clone();
        let protocols = Arc::clone(&bound.protocols);
        bound.serve.run(move |accepted| connection::<L>(accepted, service.clone(), Arc::clone(&protocols))).await
    }

    async fn drain(&self) {
        if let Some(bound) = &self.bound {
            bound.serve.drain().await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        if let Some(bound) = &self.bound {
            bound.serve.close().await;
        }
        Ok(())
    }
}

/// Polls the connection `$conn` to its end, starting its graceful shutdown once `$draining`
/// resolves: HTTP/2 sends GOAWAY, and HTTP/1.1 closes an idle connection and answers an in-flight
/// request with `Connection: close`. A macro rather than a function, since the two connection
/// types share the method but no trait hyper exports.
macro_rules! drive {
    ($conn:expr, $draining:ident, $peer:ident) => {{
        let mut conn = pin!($conn);
        let mut draining = pin!($draining.wait());
        let mut shutting_down = false;
        let result = poll_fn(|cx| {
            if let Poll::Ready(result) = conn.as_mut().poll(cx) {
                return Poll::Ready(result);
            }
            if !shutting_down && draining.as_mut().poll(cx).is_ready() {
                shutting_down = true;
                conn.as_mut().graceful_shutdown();
                return conn.as_mut().poll(cx);
            }
            Poll::Pending
        })
        .await;
        if let Err(error) = result {
            tracing::debug!(peer = ?$peer, %error, "HTTP connection ended with an error");
        }
    }};
}

/// One connection after its handshake: HTTP until the connection ends, each request converted and
/// answered by the `AppService`.
async fn connection<L: Listener>(accepted: Accepted<L>, service: AppService, protocols: Arc<Protocols>) {
    let Accepted { io, conn, draining } = accepted;
    let detect_h2 = conn.tls.is_some() || protocols.h2c;
    let peer = conn.peer;
    let service = hyper::service::service_fn(move |req: http::Request<Incoming>| {
        let reply = service.call(convert::request(req, &conn));
        async move { Ok::<_, Infallible>(reply.await) }
    });
    if detect_h2 {
        drive!(protocols.auto.serve_connection_with_upgrades(io, service), draining, peer);
    } else {
        drive!(protocols.http1.serve_connection(io, service).with_upgrades(), draining, peer);
    }
}
