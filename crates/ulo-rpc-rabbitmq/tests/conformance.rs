//! The RPC conformance suite over the RabbitMQ link, each scenario against a RabbitMQ broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo_rpc_conformance::Broker;
use ulo_rpc_conformance::relay::{Relay, reachable};
use ulo_rpc_rabbitmq::RabbitMq;

const PORT: u16 = 5672;

struct RabbitMqBroker {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RabbitMqBroker {
    type Link = RabbitMq;

    async fn start() -> Self {
        let container = GenericImage::new("rabbitmq", "4.1-alpine")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Server startup complete"))
            .start()
            .await
            .expect("the RabbitMQ container starts");
        let port = container.get_host_port_ipv4(PORT).await.expect("the AMQP port is mapped");
        let server = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        reachable(server, Duration::from_secs(10)).await;
        RabbitMqBroker { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.server))
    }

    fn client_link(&self) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }
}

ulo_rpc_conformance::conformance_suite!(RabbitMqBroker);
