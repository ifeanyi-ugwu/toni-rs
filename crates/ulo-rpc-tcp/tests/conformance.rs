//! The RPC conformance suite over the TCP link, each scenario against a server on a port of its
//! own, the client reaching it through a relay that `disrupt` cuts.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};
use ulo_rpc_conformance::Broker;
use ulo_rpc_tcp::Tcp;

struct Loopback {
    server: SocketAddr,
    relay: SocketAddr,
    /// One task per relayed connection, holding both of its sockets.
    connections: Arc<Mutex<JoinSet<()>>>,
    accepting: JoinHandle<()>,
}

impl Broker for Loopback {
    type Link = Tcp;

    /// A port the OS hands out and this process releases before the server binds it, so the
    /// relay can name it, and the relay listening on a port of its own.
    async fn start() -> Self {
        let probe = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port is free");
        let server = probe.local_addr().expect("the probe socket has an address");
        drop(probe);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("the relay binds a loopback port");
        let relay = listener.local_addr().expect("the relay has an address");
        let connections = Arc::new(Mutex::new(JoinSet::new()));
        let accepting = tokio::spawn(accept(listener, server, Arc::clone(&connections)));
        Loopback { server, relay, connections, accepting }
    }

    fn link(&self) -> Tcp {
        Tcp::new(self.server)
    }

    fn client_link(&self) -> Tcp {
        Tcp::new(self.relay)
    }

    /// Closes every connection through the relay, which keeps accepting new ones.
    async fn disrupt(&self) {
        let mut connections = self.connections.lock().await;
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// Relays each connection the client opens to the server. A server refusing the relay's connect
/// closes the client's connection, as a refused connect would fail the client's.
async fn accept(listener: TcpListener, server: SocketAddr, connections: Arc<Mutex<JoinSet<()>>>) {
    while let Ok((mut client, _)) = listener.accept().await {
        let mut connections = connections.lock().await;
        while connections.try_join_next().is_some() {}
        connections.spawn(async move {
            let Ok(mut upstream) = TcpStream::connect(server).await else { return };
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
    }
}

ulo_rpc_conformance::conformance_suite!(Loopback);
