use std::convert::Infallible;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use ulo::BoxError;
use ulo_http::{AppService, Backend, BackendLimits, HttpConfig};
use ulo_hyper_serve::{Accepted, Serve, ServeConfig};
use ulo_net::{BoundListener, TlsAcceptor};
use ulo_transport::Count;

use crate::convert;

/// The hyper backend. `Default`, so `ulo_http_hyper::Server::new(endpoint)` builds it.
///
/// `ulo-hyper-serve` accepts on every listener, one task per connection, and hands each connection
/// here after its TLS handshake, where hyper's connection builders serve it with the server's
/// settings applied.
#[derive(Default)]
pub struct Hyper {
    /// Set by `bind`.
    pub(crate) bound: Option<Bound>,
}

/// What `bind` prepared for `serve`.
pub(crate) struct Bound {
    pub(crate) serve: Serve,
    pub(crate) service: AppService,
    pub(crate) protocols: Arc<Protocols>,
}

/// How each connection is served, from the server's settings.
pub(crate) struct Protocols {
    /// HTTP/1.1, or HTTP/2 when the connection opens with its preface: a TLS connection, whose
    /// protocol ALPN settled, and a plain one when h2c is on.
    auto: auto::Builder<TokioExecutor>,
    /// HTTP/1.1 alone, for a plain connection when h2c is off, since the auto builder accepts the
    /// HTTP/2 preface on any connection it serves with upgrades.
    http1: http1::Builder,
    h2c: bool,
}

impl Protocols {
    /// `header_timeout` is hyper's HTTP/1.1 header-read timeout, set on both builders in every
    /// case, so the 30 seconds at `Bound::Default` is the server's value rather than hyper's and
    /// `Bound::Unbounded` clears it. The timer is what hyper reads that clock from; hyper panics
    /// on a header-read timeout configured without one. HTTP/2 has no head-read clock in hyper.
    ///
    /// `max_concurrent_streams` at `Count::Default` leaves hyper's own value; `Count::Unlimited`
    /// clears it, which sends no `SETTINGS_MAX_CONCURRENT_STREAMS`.
    fn new(cfg: &HttpConfig) -> Self {
        let header_timeout = cfg.header_timeout_after();
        let mut auto = auto::Builder::new(TokioExecutor::new());
        auto.http1().timer(TokioTimer::new()).header_read_timeout(header_timeout);
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
        http1.timer(TokioTimer::new()).header_read_timeout(header_timeout);
        Protocols { auto, http1, h2c: cfg.h2c }
    }
}

impl Backend for Hyper {
    const NAME: &'static str = "hyper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(
        &mut self,
        listeners: Vec<BoundListener>,
        tls: Option<TlsAcceptor>,
        svc: AppService,
        cfg: &HttpConfig,
    ) -> Result<(), BoxError> {
        let config = ServeConfig { handshake_timeout: cfg.handshake_timeout_after() };
        let serve = Serve::new(listeners, tls, &config)?;
        self.bound = Some(Bound { serve, service: svc, protocols: Arc::new(Protocols::new(cfg)) });
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(bound) = &self.bound else {
            return Err("the hyper backend was asked to serve before it was bound".into());
        };
        let service = bound.service.clone();
        let protocols = Arc::clone(&bound.protocols);
        bound.serve.run(move |accepted| connection(accepted, service.clone(), Arc::clone(&protocols))).await
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
        let mut conn = std::pin::pin!($conn);
        let mut shutting_down = false;
        loop {
            tokio::select! {
                result = conn.as_mut() => {
                    if let Err(error) = result {
                        tracing::debug!(peer = ?$peer, %error, "HTTP connection ended with an error");
                    }
                    break;
                }
                () = $draining.wait(), if !shutting_down => {
                    shutting_down = true;
                    conn.as_mut().graceful_shutdown();
                }
            }
        }
    }};
}

/// One connection after its handshake: HTTP until the connection ends, each request converted and
/// answered by the `AppService`.
async fn connection(accepted: Accepted, service: AppService, protocols: Arc<Protocols>) {
    let Accepted { io, conn, mut draining } = accepted;
    let detect_h2 = conn.tls.is_some() || protocols.h2c;
    let peer = conn.peer;
    let service = hyper::service::service_fn(move |req: http::Request<Incoming>| {
        let reply = service.call(convert::request(req, &conn));
        async move { Ok::<_, Infallible>(reply.await) }
    });
    let io = TokioIo::new(io);
    if detect_h2 {
        drive!(protocols.auto.serve_connection_with_upgrades(io, service), draining, peer);
    } else {
        drive!(protocols.http1.serve_connection(io, service).with_upgrades(), draining, peer);
    }
}
