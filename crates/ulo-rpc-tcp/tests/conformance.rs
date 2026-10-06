//! The RPC conformance suite over the TCP link, each scenario against a server on a port of its
//! own.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use ulo_rpc_conformance::Broker;
use ulo_rpc_tcp::Tcp;

struct Loopback {
    addr: SocketAddr,
}

impl Broker for Loopback {
    type Link = Tcp;

    /// A port the OS hands out and this process releases before the server binds it, so the
    /// client's link can name it.
    async fn start() -> Self {
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port is free");
        let addr = probe.local_addr().expect("the probe socket has an address");
        Loopback { addr }
    }

    fn link(&self) -> Tcp {
        Tcp::new(self.addr)
    }

    /// The broker holds no connection of the client's to sever: the client connects to the
    /// server directly, through a link the suite builds.
    async fn disrupt(&self) {}
}

ulo_rpc_conformance::conformance_suite!(Loopback);
