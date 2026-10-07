//! The RPC conformance suite over the TCP link, each scenario against a server on a port of its
//! own, the client reaching it through a relay that `disrupt` cuts.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};

use ulo_rpc_conformance::Broker;
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_tcp::Tcp;

struct Loopback {
    server: SocketAddr,
    relay: Relay,
}

impl Broker for Loopback {
    type Link = Tcp;

    /// A port the OS hands out and this process releases before the server binds it, so the
    /// relay can name it, and the relay listening on a port of its own.
    async fn start() -> Self {
        let probe = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port is free");
        let server = probe.local_addr().expect("the probe socket has an address");
        drop(probe);
        Loopback { server, relay: Relay::start(server).await }
    }

    fn link(&self) -> Tcp {
        Tcp::new(self.server)
    }

    fn client_link(&self) -> Tcp {
        Tcp::new(self.relay.addr())
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }
}

ulo_rpc_conformance::conformance_suite!(Loopback);
