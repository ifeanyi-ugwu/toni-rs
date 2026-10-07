//! A TCP relay a [`Broker`](crate::Broker) puts between a client link and what it reaches, so
//! `disrupt` can sever the client's connections and nothing else: the server keeps its own, and
//! the relay keeps accepting, so the client's next connection goes through, at once or after an
//! outage the environment chooses.
//!
//! [`reachable`] and [`unshadowed`] serve a container's published port. On a VM-backed engine
//! (OrbStack, Docker Desktop) the engine picks that port inside its VM, without seeing the host's
//! sockets, from a range that overlaps the host's ephemeral one, and forwards it from a wildcard
//! listener on the host. A host process already listening on `127.0.0.1` at that port, a relay or
//! an editor's local server, is the more specific match, and a connection to `127.0.0.1` reaches
//! it instead of the container.

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};

use crate::report;

/// Forwards every connection made to its address to `upstream`.
pub struct Relay {
    addr: SocketAddr,
    /// One task per relayed connection, holding both of its sockets.
    connections: Arc<Mutex<JoinSet<()>>>,
    outage: Arc<StdMutex<Window>>,
    accepting: JoinHandle<()>,
}

/// The last cut: until when a connection the relay accepts is closed at once, and the moments
/// [`Outage`] reports.
#[derive(Default)]
struct Window {
    until: Option<Instant>,
    shut: Option<Instant>,
    reopened: Option<Instant>,
}

/// When the relay last held its clients out, as [`Relay::last_outage`] reports it.
#[derive(Clone, Copy, Debug)]
pub struct Outage {
    /// When every connection the cut closed had ended.
    pub shut: Instant,
    /// When the relay first relayed a connection after `shut`; `None` while it has relayed none.
    pub reopened: Option<Instant>,
}

impl Relay {
    /// A relay on a loopback port of its own, forwarding to `upstream`.
    pub async fn start(upstream: SocketAddr) -> Relay {
        Relay::listen(Relay::bind().await, upstream)
    }

    /// A loopback listener for [`Relay::listen`], for an environment that has to name the relay's
    /// address before it knows the upstream one: a Kafka broker advertising the relay as its
    /// listener, say.
    pub async fn bind() -> TcpListener {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the relay did not bind a loopback port: {}", report(&error)))
    }

    /// A relay accepting on `listener`, forwarding to `upstream`.
    pub fn listen(listener: TcpListener, upstream: SocketAddr) -> Relay {
        let addr = listener.local_addr().unwrap_or_else(|error| crate::startup_failed!("the relay's listener has no address: {}", report(&error)));
        let connections = Arc::new(Mutex::new(JoinSet::new()));
        let outage = Arc::new(StdMutex::new(Window::default()));
        let accepting = tokio::spawn(accept(listener, upstream, Arc::clone(&connections), Arc::clone(&outage)));
        Relay { addr, connections, outage, accepting }
    }

    /// The address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Closes every connection through the relay, which keeps accepting new ones, and answers how
    /// many it closed.
    pub async fn cut(&self) -> usize {
        self.cut_for(Duration::ZERO).await
    }

    /// Closes every connection through the relay and, for `outage` from now, closes each new one
    /// as it is accepted, so the client stays disconnected that long. Answers how many open
    /// connections it closed; it returns without waiting for the outage to end.
    pub async fn cut_for(&self, outage: Duration) -> usize {
        *self.window() = Window { until: Some(Instant::now() + outage), shut: None, reopened: None };
        let mut connections = self.connections.lock().await;
        connections.abort_all();
        let mut severed = 0;
        while let Some(ended) = connections.join_next().await {
            if ended.is_err_and(|error| error.is_cancelled()) {
                severed += 1;
            }
        }
        self.window().shut = Some(Instant::now());
        severed
    }

    /// How many connections the relay carries now. A relayed connection ends once both of its
    /// directions have: the client closing its socket ends it when the upstream closes its own in
    /// turn, as a broker does on reading the end of a connection.
    pub async fn open(&self) -> usize {
        let mut connections = self.connections.lock().await;
        while connections.try_join_next().is_some() {}
        connections.len()
    }

    /// The last [`cut_for`](Self::cut_for)'s outage, observed: `None` before any cut.
    pub fn last_outage(&self) -> Option<Outage> {
        let window = self.window();
        Some(Outage { shut: window.shut?, reopened: window.reopened })
    }

    fn window(&self) -> std::sync::MutexGuard<'_, Window> {
        self.outage.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// How many times [`unshadowed`] starts an environment before it fails the scenario.
const SHADOW_ATTEMPTS: usize = 5;

/// Whether a connection to `addr`, a container's port published on `127.0.0.1`, reaches some other
/// listener on this host. On macOS this binds `127.0.0.1` at that port with `SO_REUSEADDR` for a
/// moment: the engine's wildcard listener allows it, and a specific listener already there
/// refuses it. On Linux the kernel refuses a wildcard bind beside a listener on `127.0.0.1`, so the
/// shadow cannot form, and the same probe would be refused by the engine's own listener: there it
/// answers `false`.
pub fn shadowed(addr: SocketAddr) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::net::TcpListener as StdTcpListener;
        // std sets `SO_REUSEADDR` on a listener it binds on Unix.
        addr.ip() == Ipv4Addr::LOCALHOST && StdTcpListener::bind(addr).is_err()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = addr;
        false
    }
}

/// Runs `start`, which starts an environment and answers it with the loopback addresses it is
/// reached at, until no other listener on this host shadows any of them ([`shadowed`]), at most
/// five times. An environment that is shadowed is dropped, which stops its container, and each
/// retry is logged on stderr. `what` names the environment in the log and in the failure.
pub async fn unshadowed<E, F, Fut>(what: &str, mut start: F) -> (E, Vec<SocketAddr>)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = (E, Vec<SocketAddr>)>,
{
    let mut collisions = Vec::new();
    for attempt in 1..=SHADOW_ATTEMPTS {
        let (environment, addrs) = start().await;
        let shadows: Vec<SocketAddr> = addrs.iter().copied().filter(|addr| shadowed(*addr)).collect();
        if shadows.is_empty() {
            return (environment, addrs);
        }
        eprintln!(
            "{what} was published on {shadows:?}, where another listener on this host would receive its connections; starting it again (attempt {attempt} of {SHADOW_ATTEMPTS})"
        );
        collisions.extend(shadows);
        drop(environment);
    }
    crate::startup_failed!("{what} was published on a port another listener on this host holds at each of {SHADOW_ATTEMPTS} starts: {collisions:?}")
}

/// Waits until `addr` accepts a TCP connection, failing the scenario after `within`. A container's
/// forwarded port can refuse connections for a moment after the broker inside reports ready.
pub async fn reachable(addr: SocketAddr, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        match TcpStream::connect(addr).await {
            Ok(_) => return,
            Err(error) if Instant::now() >= deadline => {
                crate::startup_failed!("{addr} did not accept a connection within {within:?}: {}", report(&error))
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Relays each connection to `upstream`, or closes it at once during an outage. An upstream
/// refusing the relay's connect closes the client's connection, as a refused connect would fail
/// the client's.
async fn accept(listener: TcpListener, upstream: SocketAddr, connections: Arc<Mutex<JoinSet<()>>>, outage: Arc<StdMutex<Window>>) {
    while let Ok((mut client, _)) = listener.accept().await {
        {
            let mut window = outage.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            if window.until.is_some_and(|until| now < until) {
                continue;
            }
            if window.shut.is_some() && window.reopened.is_none() {
                window.reopened = Some(now);
            }
        }
        let mut connections = connections.lock().await;
        while connections.try_join_next().is_some() {}
        connections.spawn(async move {
            let Ok(mut upstream) = TcpStream::connect(upstream).await else { return };
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
    }
}
