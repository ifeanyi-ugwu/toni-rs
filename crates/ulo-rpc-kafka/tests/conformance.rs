//! The RPC conformance suite over the Kafka link, each scenario against a single-node KRaft broker
//! in a container of its own.
//!
//! A Kafka client connects wherever the broker's metadata advertises, not where it bootstrapped,
//! so the broker has one listener for the server and one for the client, each advertising a relay
//! bound before the container starts. `disrupt` cuts the client's relay alone and keeps it shut
//! while the recovery scenario's held call is answered: the link declares `durable_replies`, and
//! the reply waits on the reply topic for the client to reconnect.
//!
//! At most `PARALLEL` brokers run at once: one per scenario, started together, compete for the
//! host's memory, and a container that runs short can exit before its ready line.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo_rpc_conformance::relay::{Outage, Relay, reachable, unshadowed};
use ulo_rpc_conformance::{Broker, Budget, report, startup_failed};
use ulo_rpc_kafka::Kafka;

/// The listener the server's link reaches.
const SERVER_PORT: u16 = 9092;
/// The listener the client's link reaches.
const CLIENT_PORT: u16 = 9095;
/// How long `disrupt` keeps the client disconnected: longer than the recovery scenario's held call
/// takes to be answered once `disrupt` begins, three seconds.
const OUTAGE: Duration = Duration::from_secs(4);

struct KraftBroker {
    server: Relay,
    client: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for KraftBroker {
    type Link = Kafka;

    const PARALLEL: Option<NonZeroUsize> = NonZeroUsize::new(4);

    async fn start() -> Self {
        let server = Relay::bind().await;
        let client = Relay::bind().await;
        let advertised = format!(
            "SERVER://{},CLIENT://{},BROKER://localhost:9094",
            server.local_addr().unwrap_or_else(|error| startup_failed!("the server relay has no address: {}", report(&error))),
            client.local_addr().unwrap_or_else(|error| startup_failed!("the client relay has no address: {}", report(&error))),
        );
        let (container, addrs) = unshadowed("the Kafka container", || {
            let advertised = advertised.clone();
            async move {
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
                    .unwrap_or_else(|error| startup_failed!("the Kafka container did not start: {}", report(&error)));
                let mut addrs = Vec::new();
                for port in [SERVER_PORT, CLIENT_PORT] {
                    let port = container.get_host_port_ipv4(port).await
                        .unwrap_or_else(|error| startup_failed!("the Kafka listener is not mapped: {}", report(&error)));
                    addrs.push(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
                }
                (container, addrs)
            }
        })
        .await;
        for addr in &addrs {
            reachable(*addr, Duration::from_secs(10)).await;
        }
        let server = Relay::listen(server, addrs[0]);
        let client = Relay::listen(client, addrs[1]);
        KraftBroker { server, client, _container: container }
    }

    fn link(&self) -> Kafka {
        Kafka::brokers(self.server.addr().to_string())
    }

    fn client_link(&self) -> Kafka {
        Kafka::brokers(self.client.addr().to_string())
    }

    /// Holds the client out for `OUTAGE`, past the moment the recovery scenario's held call is
    /// answered, so its reply is published while the client is disconnected.
    async fn disrupt(&self) {
        let severed = self.client.cut_for(OUTAGE).await;
        assert!(severed > 0, "`disrupt` found no client connection to sever");
    }

    fn outage(&self) -> Option<Outage> {
        self.client.last_outage()
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.client.open().await)
    }

    /// A consumer group takes its partitions a few seconds after `bind`, and a record waits for
    /// the next fetch.
    fn budget(&self) -> Budget {
        Budget::new(Duration::from_secs(30), Duration::from_secs(3), Duration::from_secs(30))
    }
}

ulo_rpc_conformance::conformance_suite!(KraftBroker);
