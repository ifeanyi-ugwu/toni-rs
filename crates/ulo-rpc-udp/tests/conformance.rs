//! The RPC conformance suite over the UDP link, each scenario against a server on a port of its
//! own. The link declares unary calls alone and a 65,507-byte frame, so the streamed scenarios
//! and the binary one assert the startup refusals, and the oversized one the client's refusal
//! one byte over the datagram. The recovery scenario is declared not applicable.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use ulo_rpc_conformance::{Broker, report};
use ulo_rpc_udp::Udp;

struct Loopback {
    addr: SocketAddr,
}

impl Broker for Loopback {
    type Link = Udp;

    /// A port the OS hands out and this process releases before the server binds it, so the
    /// client's link can name it.
    async fn start() -> Self {
        let probe =
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|error| panic!("no loopback port is free: {}", report(&error)));
        let addr = probe.local_addr().unwrap_or_else(|error| panic!("the probe socket has no address: {}", report(&error)));
        Loopback { addr }
    }

    fn link(&self) -> Udp {
        Udp::new(self.addr)
    }

    /// UDP holds no connection to sever, and the recovery scenario is declared not applicable.
    async fn disrupt(&self) {}
}

ulo_rpc_conformance::conformance_suite!(Loopback; not_applicable {
    recovery_after_disrupt: "UDP holds no connection to lose",
});
