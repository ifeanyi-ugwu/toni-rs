//! The RPC conformance suite over the NATS link, each scenario against a NATS server in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo_rpc_conformance::Broker;
use ulo_rpc_conformance::relay::{Relay, reachable};
use ulo_rpc_nats::Nats;

const PORT: u16 = 4222;

struct NatsServer {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for NatsServer {
    type Link = Nats;

    async fn start() -> Self {
        let container = GenericImage::new("nats", "2.10.14")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
            .start()
            .await
            .expect("the NATS container starts");
        let port = container.get_host_port_ipv4(PORT).await.expect("the NATS port is mapped");
        let server = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        reachable(server, Duration::from_secs(10)).await;
        NatsServer { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Nats {
        Nats::url(format!("nats://{}", self.server))
    }

    fn client_link(&self) -> Nats {
        Nats::url(format!("nats://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }
}

ulo_rpc_conformance::conformance_suite!(NatsServer);
