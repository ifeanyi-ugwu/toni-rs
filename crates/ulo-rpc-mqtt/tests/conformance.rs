//! The RPC conformance suite over the MQTT v5 link, each scenario against a Mosquitto broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts. Mosquitto
//! 2.x supports MQTT v5 shared subscriptions; the image's `/mosquitto-no-auth.conf` listens on
//! every interface and allows anonymous clients, which the default configuration does not.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo_rpc_conformance::Broker;
use ulo_rpc_conformance::relay::{Relay, reachable};
use ulo_rpc_mqtt::Mqtt;

const PORT: u16 = 1883;

struct Mosquitto {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for Mosquitto {
    type Link = Mqtt;

    async fn start() -> Self {
        let container = GenericImage::new("eclipse-mosquitto", "2.0.18")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stderr(" running"))
            .with_cmd(["mosquitto", "-c", "/mosquitto-no-auth.conf"])
            .start()
            .await
            .expect("the Mosquitto container starts");
        let port = container.get_host_port_ipv4(PORT).await.expect("the MQTT port is mapped");
        let server = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        reachable(server, Duration::from_secs(10)).await;
        Mosquitto { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.server))
    }

    fn client_link(&self) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }
}

ulo_rpc_conformance::conformance_suite!(Mosquitto);
