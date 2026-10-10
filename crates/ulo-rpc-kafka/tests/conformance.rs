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
//!
//! Beside the suite, one test drives the link through `Link` directly: the server's `close`
//! commits the group's offsets on a thread of its own, so a frozen broker holds no runtime worker.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo::{App, BoundAddr, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern, ReplyTo};
use ulo_rpc::__private::ClientProbe;
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

    fn probe(link: &Kafka) -> Option<&ClientProbe> {
        Some(link.probe())
    }

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

    fn client_link(&self, _server: &[BoundAddr]) -> Kafka {
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

ulo_rpc_conformance::conformance_suite!(KraftBroker; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

/// An app with nothing in it, whose handle the server's link is prepared with.
struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// How long the runtime polling `close` is watched with the broker frozen.
const FROZEN: Duration = Duration::from_secs(3);

/// The ticks of a 50 ms timer that one second of a free runtime thread allows, at the least.
const FREE_TICKS: u64 = 20;

/// F370: the server's `close` commits the group's stored offsets synchronously, which blocks the
/// calling thread until the broker answers, or until librdkafka's request timeout once it cannot.
/// With an offset stored and the broker's container paused, `close` is polled on a current-thread
/// runtime beside a task ticking every 50 ms: the commit runs on a thread of its own, so the task
/// keeps ticking while `close` waits. The broker resumes a second after the watch ends, so a commit
/// left on the closing thread, which librdkafka holds without bound while the broker is frozen,
/// ends then and the test reports it.
#[tokio::test(flavor = "multi_thread")]
async fn close_commits_off_the_runtime_thread_polling_it() {
    const PATTERN: &str = "f370.call";
    let broker = KraftBroker::start().await;
    let app = App::builder(Empty)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut server = broker.link();
    server.prepare(&app.handle()).await.expect("the server's link prepares");
    let mut inbound = server.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
    let client = broker.client_link(&[]);
    let outbound = client.connect().await.expect("the client's link connects");
    let request = Frame::Req { id: 1, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    (outbound.send)(Pattern::from(PATTERN), request, Some(ReplyTo { id: 1 })).await.expect("the request is produced");
    let delivery = tokio::time::timeout(Duration::from_secs(60), inbound.next())
        .await
        .expect("the request did not reach the server within a minute")
        .expect("the server's inbound stream ended");
    // Stores the record's offset, which the close then commits.
    delivery.ack.ack();
    client.close().await.expect("the client's link closes");

    broker._container.pause().await.expect("the broker's container pauses");
    let ticks = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&ticks);
    let closing = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("a runtime for the close");
        runtime.block_on(async move {
            let ticker = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            });
            let started = Instant::now();
            let _ = tokio::time::timeout(FROZEN, server.close()).await;
            ticker.abort();
            started.elapsed()
        })
    });
    // Resumed whatever `close` did, so a commit holding the closing thread ends once the broker
    // answers it: frozen, librdkafka waits on a synchronous commit without bound.
    tokio::time::sleep(FROZEN + Duration::from_secs(1)).await;
    broker._container.unpause().await.expect("the broker's container resumes");
    let joined = tokio::time::timeout(Duration::from_secs(60), tokio::task::spawn_blocking(move || closing.join()))
        .await
        .expect("the closing thread did not end within a minute of the broker resuming");
    let took = joined.expect("the join completes").expect("the closing thread panicked");
    let ticked = ticks.load(Ordering::SeqCst);
    assert!(
        ticked >= FREE_TICKS,
        "the runtime polling `close` ran its other task {ticked} times in {took:?} with the broker frozen: the commit held its thread"
    );
    let _ = app.close(Signal::new("conformance")).await;
}
