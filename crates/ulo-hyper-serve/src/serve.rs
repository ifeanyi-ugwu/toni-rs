//! The accept loop, the per-connection tasks and the drain.

use std::future::{Future, poll_fn};
use std::io;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

use ulo::{BoxError, Runtime};
use ulo_http::ConnInfo;
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_transport::TaskSet;
use ulo_transport::__private::Watch;

use crate::listener::{Incoming, Io, Listener, ReadCount};

/// What the accept loop needs beyond its listeners and runtime.
#[derive(Clone, Debug, Default)]
pub struct ServeConfig {
    /// How long a connection's handshake, TLS's where the listener has it, may take before the
    /// connection is dropped; `None` lets it run until the drain. A server resolves its own
    /// `handshake_timeout: Bound` to this, as `HttpConfig::handshake_timeout_after` does.
    pub handshake_timeout: Option<Duration>,
    /// Counts the connections the server has read from. A server keeps a clone from its
    /// construction and hands one here at `bind`, so a caller holding the server's clone reads
    /// the count after the server has moved into the app.
    pub read_count: ReadCount,
}

/// One accepted connection, handed to the consumer's closure on its own task.
pub struct Accepted<L: Listener> {
    pub io: Io<L::Io>,
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
    signals: Arc<Signals>,
}

impl Draining {
    /// Resolves once the drain has started, at once when it already has.
    pub async fn wait(&self) {
        self.signals.draining.wait_for(|draining| *draining).await;
    }

    pub fn is_draining(&self) -> bool {
        self.signals.draining.read(|draining| *draining)
    }
}

/// The three signals one `Serve` raises, each once.
struct Signals {
    /// Raised by `drain`: every accept loop drops its listener and every connection's
    /// `draining` resolves.
    draining: Watch<bool>,
    /// Raised by `close`: the connections still open are aborted.
    closing: Watch<bool>,
    /// Raised once nothing is left serving: `run` has ended with every connection, or `drain` or
    /// `close` released the listeners of a `run` that never started. `drain` and `close` wait on
    /// it.
    finished: Watch<bool>,
}

impl Signals {
    fn raise(signal: &Watch<bool>) {
        signal.modify(|raised| *raised = true);
    }
}

/// The accept loop over a server's listeners. Built in a server's `bind`, run by its `serve`,
/// drained and closed by its `drain` and `close`; `run` takes `&self`, so the server holds one
/// `Serve` across the four.
pub struct Serve<L: Listener> {
    /// Set by `new`, taken by `run`, or by a `drain` or `close` arriving before it.
    listeners: Mutex<Option<Vec<L>>>,
    runtime: Arc<dyn Runtime>,
    handshake_timeout: Option<Duration>,
    read_count: ReadCount,
    signals: Arc<Signals>,
}

impl<L: Listener> Serve<L> {
    /// Adopts `listeners` through `L`, each accepting TLS under `tls`, the configuration a server's
    /// `Tls::load` built, when the server has TLS. Connections, accept loops and timeouts run on
    /// `runtime`, the app's (`Mounted::runtime()`). Fails when `L` cannot adopt a listener; call it
    /// from a server's `bind`.
    pub fn new(
        listeners: Vec<BoundListener>,
        tls: Option<Arc<ServerConfig>>,
        config: &ServeConfig,
        runtime: Arc<dyn Runtime>,
    ) -> io::Result<Serve<L>> {
        let listeners = listeners.into_iter().map(|listener| L::adopt(listener, tls.clone())).collect::<io::Result<Vec<_>>>()?;
        Ok(Serve {
            listeners: Mutex::new(Some(listeners)),
            runtime,
            handshake_timeout: config.handshake_timeout,
            read_count: config.read_count.clone(),
            signals: Arc::new(Signals { draining: Watch::new(false), closing: Watch::new(false), finished: Watch::new(false) }),
        })
    }

    /// Accepts on every listener until the drain, each connection on its own task, and returns
    /// once every connection has ended. Each accepted connection is handed to `connection` after
    /// its handshake, which the drain abandons. A second call, or one after `drain` or `close`,
    /// returns at once.
    pub async fn run<F, Fut>(&self, connection: F) -> Result<(), BoxError>
    where
        F: Fn(Accepted<L>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let taken = self.listeners.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(listeners) = taken else { return Ok(()) };
        let _finish = Finish(Arc::clone(&self.signals));
        let connection = Arc::new(connection);
        let mut loops = TaskSet::new(Arc::clone(&self.runtime) as Arc<dyn ulo::Spawn>);
        for listener in listeners {
            loops.spawn(accept_loop(
                listener,
                Arc::clone(&connection),
                Arc::clone(&self.runtime),
                self.handshake_timeout,
                self.read_count.clone(),
                Arc::clone(&self.signals),
            ));
        }
        while let Some(end) = loops.join_next().await {
            if let ulo::TaskEnd::Panicked(panic) = end {
                return Err(format!("an accept loop panicked: {panic}").into());
            }
        }
        Ok(())
    }

    /// Stops accepting, drops the listeners and resolves every connection's `draining`, then
    /// waits for the connection tasks to end.
    pub async fn drain(&self) {
        Signals::raise(&self.signals.draining);
        self.release_unserved();
        self.signals.finished.wait_for(|finished| *finished).await;
    }

    /// Aborts every connection task left and waits for the loops to end.
    pub async fn close(&self) {
        Signals::raise(&self.signals.closing);
        self.release_unserved();
        self.signals.finished.wait_for(|finished| *finished).await;
    }

    /// Closes the listeners of a `run` that never started, which no accept loop will close: an
    /// app shut down before it served, or a `listen()` closing the servers it bound before a later
    /// one failed.
    fn release_unserved(&self) {
        let unserved = self.listeners.lock().unwrap_or_else(PoisonError::into_inner).take();
        if unserved.is_some() {
            Signals::raise(&self.signals.finished);
        }
    }
}

/// Marks `run` finished however its future ends, dropped included.
struct Finish(Arc<Signals>);

impl Drop for Finish {
    fn drop(&mut self) {
        Signals::raise(&self.0.finished);
    }
}

/// What woke the accept loop.
enum Woke<T> {
    Accepted(io::Result<Incoming<T>>),
    Draining,
    Closing,
    Joined,
}

/// Accepts on one listener until the drain, then waits for its connections to end. `close` aborts
/// the connections still running, waiting for each abort to take effect, and the loop then
/// returns. Dropping the listener at the drain is what stops accepting.
async fn accept_loop<L, F, Fut>(
    listener: L,
    connection: Arc<F>,
    runtime: Arc<dyn Runtime>,
    handshake_timeout: Option<Duration>,
    read_count: ReadCount,
    signals: Arc<Signals>,
) where
    L: Listener,
    F: Fn(Accepted<L>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut connections = TaskSet::new(Arc::clone(&runtime) as Arc<dyn ulo::Spawn>);
    loop {
        let woke = {
            let busy = !connections.is_empty();
            let mut closing = pin!(signals.closing.wait_for(|closing| *closing));
            let mut draining = pin!(signals.draining.wait_for(|draining| *draining));
            let mut accepted = pin!(listener.accept());
            let mut joined = pin!(connections.join_next());
            poll_fn(|cx| {
                if closing.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Woke::Closing);
                }
                if draining.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Woke::Draining);
                }
                if let Poll::Ready(accepted) = accepted.as_mut().poll(cx) {
                    return Poll::Ready(Woke::Accepted(accepted));
                }
                if busy && joined.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Woke::Joined);
                }
                Poll::Pending
            })
            .await
        };
        match woke {
            Woke::Accepted(Ok(incoming)) => {
                connections.spawn(serve_connection(
                    incoming,
                    Arc::clone(&runtime),
                    handshake_timeout,
                    Arc::clone(&connection),
                    read_count.clone(),
                    Arc::clone(&signals),
                ));
            }
            Woke::Accepted(Err(error)) => backoff(error, &*runtime).await,
            Woke::Draining => break,
            Woke::Closing => {
                connections.abort_all();
                connections.join_all().await;
                return;
            }
            Woke::Joined => {}
        }
    }
    drop(listener);
    loop {
        let closed = {
            let mut closing = pin!(signals.closing.wait_for(|closing| *closing));
            let mut joined = pin!(connections.join_next());
            poll_fn(|cx| {
                if closing.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Some(true));
                }
                match joined.as_mut().poll(cx) {
                    Poll::Ready(None) => Poll::Ready(Some(false)),
                    Poll::Ready(Some(_)) => Poll::Ready(None),
                    Poll::Pending => Poll::Pending,
                }
            })
            .await
        };
        match closed {
            Some(true) => {
                connections.abort_all();
                connections.join_all().await;
                return;
            }
            Some(false) => return,
            None => {}
        }
    }
}

/// A connection the peer abandoned between its arrival and the accept is skipped at once. Any
/// other error, running out of file descriptors above all, would fail again immediately, so the
/// loop waits a second first, as hyper's own accept loop did.
async fn backoff(error: io::Error, timer: &dyn Runtime) {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(%error, "accepting a connection failed; retrying in one second");
    timer.sleep(Duration::from_secs(1)).await;
}

/// How a connection's handshake ended.
enum Handshaken<T> {
    Ready(io::Result<(T, Option<ulo_http::TlsInfo>)>),
    TimedOut,
    Draining,
}

/// One connection: its handshake, bounded by the handshake timeout and abandoned at the drain,
/// then the consumer's closure until the connection ends. A failed or timed-out handshake is
/// routine (scanners, clients that refuse the certificate), so it is logged at `debug` with the
/// peer and the connection dropped.
async fn serve_connection<L, F, Fut>(
    incoming: Incoming<L::Io>,
    runtime: Arc<dyn Runtime>,
    handshake_timeout: Option<Duration>,
    connection: Arc<F>,
    read_count: ReadCount,
    signals: Arc<Signals>,
) where
    L: Listener,
    F: Fn(Accepted<L>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Incoming { peer, local, handshake } = incoming;
    let outcome = {
        let mut handshake = pin!(handshake);
        let mut draining = pin!(signals.draining.wait_for(|draining| *draining));
        let mut expired = handshake_timeout.map(|timeout| runtime.sleep(timeout));
        poll_fn(|cx| {
            if let Poll::Ready(outcome) = handshake.as_mut().poll(cx) {
                return Poll::Ready(Handshaken::Ready(outcome));
            }
            if draining.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Handshaken::Draining);
            }
            if let Some(expired) = expired.as_mut()
                && expired.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Handshaken::TimedOut);
            }
            Poll::Pending
        })
        .await
    };
    let (io, tls) = match outcome {
        Handshaken::Ready(Ok(ready)) => ready,
        Handshaken::Ready(Err(error)) => {
            tracing::debug!(%peer, %error, "TLS handshake failed");
            return;
        }
        Handshaken::TimedOut => {
            tracing::debug!(%peer, ?handshake_timeout, "TLS handshake timed out");
            return;
        }
        Handshaken::Draining => return,
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
    let io = Io::counting(io, read_count);
    (*connection)(Accepted { io, conn, draining: Draining { signals } }).await;
}
