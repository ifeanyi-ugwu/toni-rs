use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use ulo::BoxError;
use ulo_http::{AppService, Backend, BackendLimits, HttpConfig};
use ulo_net::{BoundListener, TlsAcceptor};

use crate::convert::{self, Conn};
use crate::listener::{self, Io, Listener};

/// The hyper backend. `Default`, so `ulo_http_hyper::Server::new(endpoint)` builds it.
///
/// Each listener has its own accept loop, a spawned task, and each connection its own task under
/// that loop, served by hyper's connection builders with the server's settings applied.
#[derive(Default)]
pub struct Hyper {
    /// Set by `bind`, taken by `serve`.
    pub(crate) bound: Mutex<Option<Bound>>,
    /// `drain` sends `true`: every accept loop closes its listener and every connection starts its
    /// graceful shutdown, which sends GOAWAY on HTTP/2 and closes idle HTTP/1.1 keep-alive
    /// connections.
    pub(crate) draining: Option<watch::Sender<bool>>,
    /// `close` sends `true`: connections still open are dropped.
    pub(crate) closing: Option<watch::Sender<bool>>,
    /// `true` once nothing is left serving: `serve` has ended with every connection, or `drain` or
    /// `close` closed the listeners of a `serve` that never started. `drain` and `close` wait on it.
    pub(crate) finished: Option<watch::Sender<bool>>,
}

/// What `bind` prepared for `serve`.
pub(crate) struct Bound {
    pub(crate) listeners: Vec<Listener>,
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
    /// The timer turns on hyper's default HTTP/1.1 header-read timeout, 30 seconds, which hyper
    /// skips without one. `max_concurrent_streams` unset keeps hyper's default.
    fn new(cfg: &HttpConfig) -> Self {
        let mut auto = auto::Builder::new(TokioExecutor::new());
        auto.http1().timer(TokioTimer::new());
        if let Some(streams) = cfg.max_concurrent_streams {
            auto.http2().max_concurrent_streams(streams);
        }
        let mut http1 = http1::Builder::new();
        http1.timer(TokioTimer::new());
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
        let listeners = listeners
            .into_iter()
            .map(|listener| -> std::io::Result<Listener> {
                let tcp = tokio::net::TcpListener::from_std(listener.into_std())?;
                Ok(Listener { tcp, tls: tls.clone() })
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        let bound = Bound { listeners, service: svc, protocols: Arc::new(Protocols::new(cfg)) };
        *self.bound.get_mut().unwrap_or_else(PoisonError::into_inner) = Some(bound);
        self.draining = Some(watch::channel(false).0);
        self.closing = Some(watch::channel(false).0);
        self.finished = Some(watch::channel(false).0);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let (Some(draining), Some(closing), Some(finished)) = (&self.draining, &self.closing, &self.finished) else {
            return Err("the hyper backend was asked to serve before it was bound".into());
        };
        let taken = self.bound.lock().unwrap_or_else(PoisonError::into_inner).take();
        // Taken already: a `drain` or `close` that arrived first closed the listeners.
        let Some(bound) = taken else { return Ok(()) };
        let _finish = Finish(finished);
        let mut loops = JoinSet::new();
        for listener in bound.listeners {
            loops.spawn(accept_loop(
                listener,
                bound.service.clone(),
                Arc::clone(&bound.protocols),
                draining.subscribe(),
                closing.subscribe(),
            ));
        }
        while let Some(joined) = loops.join_next().await {
            if let Err(error) = joined
                && error.is_panic()
            {
                return Err(format!("an accept loop panicked: {error}").into());
            }
        }
        Ok(())
    }

    async fn drain(&self) {
        let (Some(draining), Some(finished)) = (&self.draining, &self.finished) else { return };
        draining.send_replace(true);
        self.release_unserved();
        raised(&mut finished.subscribe()).await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (Some(closing), Some(finished)) = (&self.closing, &self.finished) else { return Ok(()) };
        closing.send_replace(true);
        self.release_unserved();
        raised(&mut finished.subscribe()).await;
        Ok(())
    }
}

impl Hyper {
    /// Closes the listeners of a `serve` that never started, which no accept loop will close: an
    /// app shut down before it served, or a `listen()` closing the servers it bound before a later
    /// one failed.
    fn release_unserved(&self) {
        let unserved = self.bound.lock().unwrap_or_else(PoisonError::into_inner).take();
        if unserved.is_some()
            && let Some(finished) = &self.finished
        {
            finished.send_replace(true);
        }
    }
}

/// Marks `serve` finished however its future ends, dropped included.
struct Finish<'a>(&'a watch::Sender<bool>);

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

/// Resolves once `signal` reads `true`, or once its sender is gone.
async fn raised(signal: &mut watch::Receiver<bool>) {
    let _ = signal.wait_for(|raised| *raised).await;
}

/// Accepts on one listener until the drain, then waits for its connections to end; `close` aborts
/// whatever is left. Dropping the listener at the drain is what stops accepting.
async fn accept_loop(
    listener: Listener,
    service: AppService,
    protocols: Arc<Protocols>,
    mut draining: watch::Receiver<bool>,
    mut closing: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            (stream, peer) = listener.accept() => {
                connections.spawn(connection(
                    stream,
                    peer,
                    listener.tls.clone(),
                    service.clone(),
                    Arc::clone(&protocols),
                    draining.clone(),
                ));
            }
            () = raised(&mut draining) => break,
            () = raised(&mut closing) => {
                connections.shutdown().await;
                return;
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    loop {
        tokio::select! {
            joined = connections.join_next() => {
                if joined.is_none() {
                    return;
                }
            }
            () = raised(&mut closing) => {
                connections.shutdown().await;
                return;
            }
        }
    }
}

/// Polls the connection `$conn` to its end, starting its graceful shutdown once `$draining` is
/// raised: HTTP/2 sends GOAWAY, and HTTP/1.1 closes an idle connection and answers an in-flight
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
                        tracing::debug!(peer = %$peer, %error, "HTTP connection ended with an error");
                    }
                    break;
                }
                () = raised(&mut $draining), if !shutting_down => {
                    shutting_down = true;
                    conn.as_mut().graceful_shutdown();
                }
            }
        }
    }};
}

/// One connection: the TLS handshake when the listener has TLS, abandoned at the drain, then HTTP
/// until the connection ends, each request converted and answered by the `AppService`.
async fn connection(
    stream: TcpStream,
    peer: SocketAddr,
    tls: Option<TlsAcceptor>,
    service: AppService,
    protocols: Arc<Protocols>,
    mut draining: watch::Receiver<bool>,
) {
    let local = stream.local_addr().ok();
    let (io, tls) = match tls {
        None => (Io::Plain(stream), None),
        Some(acceptor) => {
            let outcome = tokio::select! {
                outcome = listener::handshake(&acceptor, stream, peer) => outcome,
                () = raised(&mut draining) => return,
            };
            match outcome {
                Some((io, info)) => (io, Some(info)),
                None => return,
            }
        }
    };
    let detect_h2 = tls.is_some() || protocols.h2c;
    let conn = Conn { peer, local, tls };
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
