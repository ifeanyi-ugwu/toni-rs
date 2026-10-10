//! The RPC conformance suite over the TCP link, each scenario against a server on a port of its
//! own, the client reaching it through a relay that `disrupt` cuts. Beside it, the link-level check
//! that a `cancel` reaches the server after its request, one connection carrying both.

use std::net::{Ipv4Addr, SocketAddr};

use ulo::BoundAddr;
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Broker, startup_failed};
use ulo_rpc_tcp::Tcp;

struct Loopback {
    relay: Relay,
}

impl Broker for Loopback {
    type Link = Tcp;

    /// The relay, listening on a port of its own; it learns the server's address from
    /// `client_link`, once the server is bound.
    async fn start() -> Self {
        Loopback { relay: Relay::without_upstream().await }
    }

    /// Port 0: the OS chooses the server's port as it binds.
    fn link(&self) -> Tcp {
        Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }

    /// Through the relay, aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Tcp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        self.relay.forward_to(bound.addr);
        Tcp::new(self.relay.addr())
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(Loopback);

#[tokio::test(flavor = "multi_thread")]
async fn cancel_follows_its_request() {
    ulo_rpc_conformance::cases::order::cancel_follows_its_request::<Loopback>().await;
}
