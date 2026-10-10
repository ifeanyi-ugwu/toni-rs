//! The RPC conformance suite over the RabbitMQ link, each scenario against a RabbitMQ broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::BoundAddr;
use ulo_rpc::__private::ClientProbe;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc_rabbitmq::RabbitMq;

const PORT: u16 = 5672;

struct RabbitMqBroker {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RabbitMqBroker {
    type Link = RabbitMq;

    fn probe(link: &RabbitMq) -> Option<&ClientProbe> {
        Some(link.probe())
    }

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the RabbitMQ container", || async {
            let container = GenericImage::new("rabbitmq", "4.1-alpine")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Server startup complete"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the RabbitMQ container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the AMQP port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        RabbitMqBroker { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(RabbitMqBroker; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});
