//! The RPC conformance suite over the TCP link speaking CBOR, each scenario against a server on a
//! port of its own, the client reaching it through a relay that `disrupt` cuts. The suite's other
//! stamps run on JSON links, so this one is where a payload encoded with a codec other than the
//! link's fails.

use std::net::{Ipv4Addr, SocketAddr};

use ulo::BoundAddr;
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Broker, startup_failed};
use ulo_rpc::Codec;
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
        Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).codec(Codec::Cbor)
    }

    /// Through the relay, aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Tcp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        self.relay.forward_to(bound.addr);
        Tcp::new(self.relay.addr()).codec(Codec::Cbor)
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(Loopback; not_applicable {
    a_late_opened_releases_the_held_cancel: "U17: a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
    a_held_cancel_is_dropped_when_its_hold_runs_out: "U17: a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
});
