//! The accept loop, the per-connection tasks and the drain.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use ulo::BoxError;
use ulo_http::ConnInfo;
use ulo_net::BoundListener;

use crate::handshake::handshake;
use crate::listener::{Inner, Io, Listener};

/// What the accept loop needs beyond its listeners.
#[derive(Clone, Debug, Default)]
pub struct ServeConfig {
    /// How long a TLS handshake may take before the connection is dropped; `None` lets it run
    /// until the drain. A server resolves its own `handshake_timeout: Bound` to this, as
    /// `HttpConfig::handshake_timeout_after` does.
    pub handshake_timeout: Option<Duration>,
}

/// One accepted connection, handed to the consumer's closure on its own task.
pub struct Accepted {
    pub io: Io,
    /// The peer and local addresses and what the TLS handshake settled. `version` is HTTP/2 when
    /// ALPN settled `h2` and HTTP/1.1 otherwise; a consumer building a request's `ConnInfo` sets
    /// the request's own version.
    pub conn: ConnInfo,
    /// Resolves when the drain starts: the closure starts its connection's graceful shutdown then.
    pub draining: Draining,
}

/// The drain's signal as one connection sees it.
#[derive(Clone)]
pub struct Draining {
    rx: watch::Receiver<bool>,
}

impl Draining {
    /// Resolves once the drain has started, at once when it already has.
    pub async fn wait(&mut self) {
        raised(&mut self.rx).await;
    }

    pub fn is_draining(&self) -> bool {
        *self.rx.borrow()
    }
}

/// The accept loop over a server's listeners. Built in a server's `bind`, inside the runtime,
/// run by its `serve`, drained and closed by its `drain` and `close`; `run` takes `&self`, so the
/// server holds one `Serve` across the four.
pub struct Serve {
    /// Set by `new`, taken by `run`, or by a `drain` or `close` arriving before it.
    listeners: Mutex<Option<Vec<Listener>>>,
    handshake_timeout: Option<Duration>,
    /// `drain` sends `true`: every accept loop drops its listener and every connection's
    /// `draining` resolves.
    draining: watch::Sender<bool>,
    /// `close` sends `true`: the connections still open are aborted.
    closing: watch::Sender<bool>,
    /// `true` once nothing is left serving: `run` has ended with every connection, or `drain` or
    /// `close` released the listeners of a `run` that never started. `drain` and `close` wait on it.
    finished: watch::Sender<bool>,
}

impl Serve {
    /// Adopts `listeners` into the runtime, each with `tls` when the server has TLS. Fails when
    /// the runtime cannot register a listener; call it inside the runtime, from a server's `bind`.
    pub fn new(listeners: Vec<BoundListener>, tls: Option<TlsAcceptor>, config: &ServeConfig) -> io::Result<Serve> {
        let listeners = listeners
            .into_iter()
            .map(|listener| -> io::Result<Listener> {
                let tcp = tokio::net::TcpListener::from_std(listener.into_std())?;
                Ok(Listener { tcp, tls: tls.clone() })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Serve {
            listeners: Mutex::new(Some(listeners)),
            handshake_timeout: config.handshake_timeout,
            draining: watch::channel(false).0,
            closing: watch::channel(false).0,
            finished: watch::channel(false).0,
        })
    }

    /// Accepts on every listener until the drain, each connection on its own task, and returns
    /// once every connection has ended. Each accepted connection is handed to `connection` after
    /// its TLS handshake, which the drain abandons. A second call, or one after `drain` or `close`,
    /// returns at once.
    pub async fn run<F, Fut>(&self, connection: F) -> Result<(), BoxError>
    where
        F: Fn(Accepted) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let taken = self.listeners.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(listeners) = taken else { return Ok(()) };
        let _finish = Finish(&self.finished);
        let connection = Arc::new(connection);
        let mut loops = JoinSet::new();
        for listener in listeners {
            loops.spawn(accept_loop(
                listener,
                Arc::clone(&connection),
                self.handshake_timeout,
                self.draining.subscribe(),
                self.closing.subscribe(),
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

    /// Stops accepting, drops the listeners and resolves every connection's `draining`, then
    /// waits for the connection tasks to end.
    pub async fn drain(&self) {
        self.draining.send_replace(true);
        self.release_unserved();
        raised(&mut self.finished.subscribe()).await;
    }

    /// Aborts every connection task left and waits for the loops to end.
    pub async fn close(&self) {
        self.closing.send_replace(true);
        self.release_unserved();
        raised(&mut self.finished.subscribe()).await;
    }

    /// Closes the listeners of a `run` that never started, which no accept loop will close: an
    /// app shut down before it served, or a `listen()` closing the servers it bound before a later
    /// one failed.
    fn release_unserved(&self) {
        let unserved = self.listeners.lock().unwrap_or_else(PoisonError::into_inner).take();
        if unserved.is_some() {
            self.finished.send_replace(true);
        }
    }
}

/// Marks `run` finished however its future ends, dropped included.
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

/// Accepts on one listener until the drain, then waits for its connections to end. `close` aborts
/// the connections still running, `JoinSet::shutdown` awaiting each abort, and the loop then
/// returns. Dropping the listener at the drain is what stops accepting.
async fn accept_loop<F, Fut>(
    listener: Listener,
    connection: Arc<F>,
    handshake_timeout: Option<Duration>,
    mut draining: watch::Receiver<bool>,
    mut closing: watch::Receiver<bool>,
) where
    F: Fn(Accepted) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            (stream, peer) = listener.accept() => {
                connections.spawn(serve_connection(
                    stream,
                    peer,
                    listener.tls.clone(),
                    handshake_timeout,
                    Arc::clone(&connection),
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

/// One connection: the TLS handshake when the listener has TLS, abandoned at the drain, then the
/// consumer's closure until the connection ends.
async fn serve_connection<F, Fut>(
    stream: TcpStream,
    peer: SocketAddr,
    tls: Option<TlsAcceptor>,
    handshake_timeout: Option<Duration>,
    connection: Arc<F>,
    mut draining: watch::Receiver<bool>,
) where
    F: Fn(Accepted) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let local = stream.local_addr().ok();
    let (io, tls) = match tls {
        None => (Io(Inner::Plain(stream)), None),
        Some(acceptor) => {
            let outcome = tokio::select! {
                outcome = handshake(&acceptor, stream, peer, handshake_timeout) => outcome,
                () = raised(&mut draining) => return,
            };
            match outcome {
                Some((io, info)) => (io, Some(info)),
                None => return,
            }
        }
    };
    let h2 = tls.as_ref().and_then(|info| info.alpn.as_deref()) == Some(b"h2".as_slice());
    let version = if h2 { http::Version::HTTP_2 } else { http::Version::HTTP_11 };
    let mut conn = ConnInfo::new(version).peer(peer);
    if let Some(local) = local {
        conn = conn.local(local);
    }
    if let Some(info) = tls {
        conn = conn.tls(info);
    }
    (*connection)(Accepted { io, conn, draining: Draining { rx: draining } }).await;
}
