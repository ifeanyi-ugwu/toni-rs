//! The RPC conformance suite over the UDP link, each scenario against a server on a port of its
//! own. The link declares unary calls alone and a 65,507-byte frame, so the streamed scenarios
//! and the binary one assert the startup refusals, and the oversized one the client's refusal
//! one byte over the datagram.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use ulo_rpc_conformance::Broker;
use ulo_rpc_udp::Udp;

struct Loopback {
    addr: SocketAddr,
}

impl Broker for Loopback {
    type Link = Udp;

    /// A port the OS hands out and this process releases before the server binds it, so the
    /// client's link can name it.
    async fn start() -> Self {
        let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port is free");
        let addr = probe.local_addr().expect("the probe socket has an address");
        Loopback { addr }
    }

    fn link(&self) -> Udp {
        Udp::new(self.addr)
    }

    /// UDP holds no connection to sever.
    async fn disrupt(&self) {}
}

ulo_rpc_conformance::conformance_suite!(Loopback);
