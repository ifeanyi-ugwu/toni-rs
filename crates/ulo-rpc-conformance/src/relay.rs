//! A TCP relay a [`Broker`](crate::Broker) puts between a client link and what it reaches, so
//! `disrupt` can sever the client's connections and nothing else: the server keeps its own, and
//! the relay keeps accepting, so the client's next connection goes through, at once or after an
//! outage the environment chooses.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};

/// Forwards every connection made to its address to `upstream`.
pub struct Relay {
    addr: SocketAddr,
    /// One task per relayed connection, holding both of its sockets.
    connections: Arc<Mutex<JoinSet<()>>>,
    /// Until when a connection the relay accepts is closed at once.
    outage: Arc<StdMutex<Option<Instant>>>,
    accepting: JoinHandle<()>,
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
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("the relay binds a loopback port")
    }

    /// A relay accepting on `listener`, forwarding to `upstream`.
    pub fn listen(listener: TcpListener, upstream: SocketAddr) -> Relay {
        let addr = listener.local_addr().expect("the relay has an address");
        let connections = Arc::new(Mutex::new(JoinSet::new()));
        let outage = Arc::new(StdMutex::new(None));
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
        *self.outage.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now() + outage);
        let mut connections = self.connections.lock().await;
        connections.abort_all();
        let mut severed = 0;
        while let Some(ended) = connections.join_next().await {
            if ended.is_err_and(|error| error.is_cancelled()) {
                severed += 1;
            }
        }
        severed
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// Waits until `addr` accepts a TCP connection, failing the scenario after `within`. A container's
/// forwarded port can refuse connections for a moment after the broker inside reports ready.
pub async fn reachable(addr: SocketAddr, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        match TcpStream::connect(addr).await {
            Ok(_) => return,
            Err(error) if Instant::now() >= deadline => panic!("{addr} did not accept a connection within {within:?}: {error}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Relays each connection to `upstream`, or closes it at once during an outage. An upstream
/// refusing the relay's connect closes the client's connection, as a refused connect would fail
/// the client's.
async fn accept(listener: TcpListener, upstream: SocketAddr, connections: Arc<Mutex<JoinSet<()>>>, outage: Arc<StdMutex<Option<Instant>>>) {
    while let Ok((mut client, _)) = listener.accept().await {
        let out = outage.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|until| Instant::now() < until);
        if out {
            continue;
        }
        let mut connections = connections.lock().await;
        while connections.try_join_next().is_some() {}
        connections.spawn(async move {
            let Ok(mut upstream) = TcpStream::connect(upstream).await else { return };
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
    }
}
