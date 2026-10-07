//! The RPC conformance suite over the UDP link, each scenario against a server on a port of its
//! own. The link declares unary calls alone and a 65,507-byte frame, so the streamed scenarios
//! and the binary one assert the startup refusals, and the oversized one the client's refusal
//! one byte over the datagram. The recovery scenario is declared not applicable.

use std::net::{Ipv4Addr, SocketAddr};

use ulo::BoundAddr;
use ulo_rpc_conformance::{Broker, startup_failed};
use ulo_rpc_udp::Udp;

struct Loopback;

impl Broker for Loopback {
    type Link = Udp;

    async fn start() -> Self {
        Loopback
    }

    /// Port 0: the OS chooses the server's port as it binds.
    fn link(&self) -> Udp {
        Udp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }

    /// Aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Udp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        Udp::new(bound.addr)
    }

    /// UDP holds no connection to sever, and the recovery scenario is declared not applicable.
    async fn disrupt(&self) {}
}

ulo_rpc_conformance::conformance_suite!(Loopback; not_applicable {
    recovery_after_disrupt: "UDP holds no connection to lose",
});
