//! The RPC conformance suite over the Kafka link, each scenario against a single-node KRaft broker
//! in a container of its own.
//!
//! A Kafka client connects wherever the broker's metadata advertises, not where it bootstrapped,
//! so the broker has one listener for the server and one for the client, each advertising a relay
//! bound before the container starts. `disrupt` cuts the client's relay alone, which the
//! recovery scenario, declared not applicable, would need to cost the waiting call its reply.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo_rpc_conformance::relay::{Relay, reachable};
use ulo_rpc_conformance::{Broker, Budget};
use ulo_rpc_kafka::Kafka;

/// The listener the server's link reaches.
const SERVER_PORT: u16 = 9092;
/// The listener the client's link reaches.
const CLIENT_PORT: u16 = 9095;

struct KraftBroker {
    server: Relay,
    client: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for KraftBroker {
    type Link = Kafka;

    async fn start() -> Self {
        let server = Relay::bind().await;
        let client = Relay::bind().await;
        let advertised = format!(
            "SERVER://{},CLIENT://{},BROKER://localhost:9094",
            server.local_addr().expect("the server relay has an address"),
            client.local_addr().expect("the client relay has an address"),
        );
        let container = GenericImage::new("apache/kafka-native", "3.8.0")
            .with_exposed_port(SERVER_PORT.tcp())
            .with_exposed_port(CLIENT_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Kafka Server started"))
            .with_env_var("CLUSTER_ID", "ulo-rpc-conformance-kafka")
            .with_env_var("KAFKA_NODE_ID", "1")
            .with_env_var("KAFKA_PROCESS_ROLES", "broker,controller")
            .with_env_var("KAFKA_CONTROLLER_QUORUM_VOTERS", "1@localhost:9093")
            .with_env_var("KAFKA_CONTROLLER_LISTENER_NAMES", "CONTROLLER")
            .with_env_var("KAFKA_LISTENERS", "SERVER://:9092,CLIENT://:9095,CONTROLLER://:9093,BROKER://:9094")
            .with_env_var("KAFKA_ADVERTISED_LISTENERS", advertised)
            .with_env_var(
                "KAFKA_LISTENER_SECURITY_PROTOCOL_MAP",
                "SERVER:PLAINTEXT,CLIENT:PLAINTEXT,CONTROLLER:PLAINTEXT,BROKER:PLAINTEXT",
            )
            .with_env_var("KAFKA_INTER_BROKER_LISTENER_NAME", "BROKER")
            .with_env_var("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR", "1")
            .with_env_var("KAFKA_OFFSETS_TOPIC_NUM_PARTITIONS", "1")
            .with_env_var("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR", "1")
            .with_env_var("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR", "1")
            // A group's first join otherwise waits three seconds for more members.
            .with_env_var("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS", "0")
            .start()
            .await
            .expect("the Kafka container starts");
        let upstream = async |port: u16| {
            let port = container.get_host_port_ipv4(port).await.expect("the Kafka listener is mapped");
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            reachable(addr, Duration::from_secs(10)).await;
            addr
        };
        let server = Relay::listen(server, upstream(SERVER_PORT).await);
        let client = Relay::listen(client, upstream(CLIENT_PORT).await);
        KraftBroker { server, client, _container: container }
    }

    fn link(&self) -> Kafka {
        Kafka::brokers(self.server.addr().to_string())
    }

    fn client_link(&self) -> Kafka {
        Kafka::brokers(self.client.addr().to_string())
    }

    async fn disrupt(&self) {
        self.client.cut().await;
    }

    /// A consumer group takes its partitions a few seconds after `bind`, and a record waits for
    /// the next fetch.
    fn budget(&self) -> Budget {
        Budget::new(Duration::from_secs(30), Duration::from_secs(3), Duration::from_secs(30))
    }
}

ulo_rpc_conformance::conformance_suite!(KraftBroker; not_applicable {
    recovery_after_disrupt: "the reply topic is durable and librdkafka reconnects by itself, so a severed connection loses no reply and the waiting call is answered",
});
