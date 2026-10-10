use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer, StreamConsumer};
use rdkafka::error::KafkaError;
use rdkafka::message::{Header, Headers, Message, OwnedHeaders, OwnedMessage};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::types::RDKafkaErrorCode;
use rdkafka::util::Timeout;
use rdkafka::{Offset, TopicPartitionList};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use ulo::{AppHandle, BoxError, BoxFuture, Timer};
use ulo_tokio::Tokio;
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, Frame, FrameTooLarge, Link, NoDestination,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::__private::ordered;
use ulo_transport::Count;

/// The header naming a record's frame kind where the record alone does not tell it: `open` on a
/// pattern's topic, `in`, `in_end` and `cancel` on the control topic, `opened` on the reply topic.
/// A record on a pattern's topic without it is a `req` when it carries `ulo-reply-to` and an `evt`
/// when it does not.
const KIND: &str = "ulo-t";
const REPLY_TO: &str = "ulo-reply-to";
const CORRELATION: &str = "ulo-correlation-id";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The topic every server instance reads from its end, outside the consumer group, so each sees
/// every control record and acts on the ones whose correlation id names a call it holds. A
/// streamed request's items and every `cancel` travel here: the group hands a pattern's partition
/// to one instance, and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// The producer's `message.max.bytes`, librdkafka's default, written out so the link can declare
/// it as its `max_frame`. A broker configured lower refuses with `MSG_SIZE_TOO_LARGE`, which the
/// link reports the same way.
const MAX_MESSAGE: u64 = 1_000_000;

/// How long librdkafka keeps trying to deliver one record, in place of its five-minute default.
const DELIVERY_TIMEOUT: &str = "30000";

/// How long a record waits for room in librdkafka's queue before its produce fails.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

/// Frames queued for one side's writer ahead of the next send's wait: each send is queued at its call and waits until it is
/// among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// The Kafka link.
///
/// Each side produces through one writer task, fed in order by every send on it, so the records a
/// side sends are queued with librdkafka in the order they were sent: a request's `in` items
/// before its `in_end`, and a `cancel` after the request it names. A request's topic and the
/// control topic are separate, so the broker can still deliver a `cancel` ahead of its request.
///
/// The link's clients and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
pub struct Kafka {
    pub(crate) brokers: String,
    pub(crate) runtime: Option<Tokio>,
    pub(crate) group: Option<String>,
    pub(crate) reply_topic: Option<String>,
    pub(crate) codec: Codec,
    pub(crate) partitions: i32,
    pub(crate) replication: i32,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The server's in-flight bound, from `Link::max_inflight`; `None` for no bound.
    pub(crate) max_inflight: Option<usize>,
    /// The app's clock, from `prepare`, which times the anchor's retries.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<Arc<ServerSide>>,
    client: Option<Arc<ClientSide>>,
}

impl Kafka {
    /// The link on `brokers`, a comma-separated `host:port` list, connected lazily. An entry
    /// written `ssl://host:port` selects `security.protocol=SSL`, which the crate's `tls` feature
    /// compiles in.
    pub fn brokers(brokers: impl Into<String>) -> Self {
        Kafka {
            brokers: brokers.into(),
            runtime: Tokio::try_current(),
            group: None,
            reply_topic: None,
            codec: Codec::Json,
            partitions: 1,
            replication: 1,
            default_group: None,
            max_inflight: Count::Default.max_inflight(),
            timer: None,
            state: Mutex::new(State::default()),
        }
    }

    /// The consumer group server instances share, in place of the root module's full type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The client's reply topic, one per logical client and reused across restarts; unset, one per
    /// process.
    pub fn reply_topic(mut self, topic: impl Into<String>) -> Self {
        self.reply_topic = Some(topic.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// Partitions for each topic the link creates, a handler's topic at `bind` and the client's
    /// reply topic; 1 unset. A topic that already exists keeps its own.
    pub fn topic_partitions(mut self, partitions: i32) -> Self {
        self.partitions = partitions;
        self
    }

    /// The replication factor for each topic the link creates; 1 unset. A topic that already
    /// exists keeps its own.
    pub fn replication_factor(mut self, replication: i32) -> Self {
        self.replication = replication;
        self
    }

    /// The tokio runtime the link's clients and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the Kafka link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Kafka {
    const NAME: &'static str = "kafka";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerPartition)
            .native_backpressure(true)
            .holds_unserved(true)
            .durable_replies(true)
            .max_frame(Some(MAX_MESSAGE))
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.usable()?;
        Brokers::parse(&self.brokers)?;
        if self.partitions < 1 || self.replication < 1 {
            return Err(format!(
                "the Kafka link's `topic_partitions` ({}) and `replication_factor` ({}) must each be at least 1",
                self.partitions, self.replication
            )
            .into());
        }
        match &self.group {
            Some(group) if group.is_empty() => return Err("the Kafka link's group is empty".into()),
            Some(_) => {}
            None => self.default_group = Some(format!("{:#}", app.root().name())),
        }
        self.timer = app.timer().cloned();
        Ok(())
    }

    /// The bound at which the request lane pauses its partitions, so a request over it waits in
    /// its topic: `Count::max_inflight`'s reading, 1,024 at `Default`.
    fn max_inflight(&mut self, calls: Count) {
        self.max_inflight = calls.max_inflight();
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the Kafka link's group is unset: `listen` ran before `prepare`")?;
        let runtime = self.runtime()?;
        let base = Brokers::parse(&self.brokers)?.config();
        let timer = self.timer.clone().ok_or("the Kafka link has no timer: `listen` ran before `prepare`, or on an app with none")?;
        let topics: Vec<String> = patterns.iter().map(|pattern| pattern.as_str().to_owned()).collect();
        let (partitions, replication) = (self.partitions, self.replication);
        let (consumer, control, producer) = runtime
            .run(async move {
                // Created here, not on the first produce, so a stopped server's topic still exists
                // and a miss is the caller's `Timeout` whether or not the broker auto-creates
                // topics; the consumer also gets its partitions at once rather than after a
                // metadata refresh.
                create_topics(&base, &topics, partitions, replication).await?;
                create_topics(&base, &[CONTROL.to_owned()], 1, replication).await?;
                anchor(&base, &group, &topics, &timer).await?;

                let mut config = base.clone();
                config
                    .set("group.id", &group)
                    .set("enable.auto.commit", "true")
                    .set("enable.auto.offset.store", "false")
                    .set("auto.offset.reset", "latest");
                let consumer: Arc<Detached<StreamConsumer>> = Arc::new(Detached::new(config.create()?));
                let names: Vec<&str> = topics.iter().map(String::as_str).collect();
                consumer.subscribe(&names)?;

                let control = control_consumer(&base, &group).await?;
                let producer = producer(&base)?;
                Ok::<_, BoxError>((consumer, control, producer))
            })
            .await??;

        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        runtime.handle().spawn(server_writer(producer, queued));
        let side = Arc::new(ServerSide {
            consumer,
            writes,
            codec: self.codec,
            runtime: runtime.clone(),
            calls: Arc::new(Calls::new()),
            phase,
            deliveries: Mutex::new(Some(deliveries)),
            inflight: Arc::new(watch::channel(0).0),
            limit: self.max_inflight,
        });
        runtime.handle().spawn(request_lane(Arc::clone(&side)));
        runtime.handle().spawn(control_lane(Arc::clone(&side), control));
        lock(&self.state).server = Some(side);
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let base = Brokers::parse(&self.brokers)?.config();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let reply_topic = self.reply_topic.clone().unwrap_or_else(|| format!("ulo.rpc.reply.{id}"));
        let (partitions, replication) = (self.partitions, self.replication);
        let (replies, producer) = {
            let reply_topic = reply_topic.clone();
            runtime
                .run(async move {
                    create_topics(&base, std::slice::from_ref(&reply_topic), partitions, replication).await?;

                    // One group per reply topic reads every partition of it; `earliest` keeps a
                    // reply that lands before the group's first assignment.
                    let mut config = base.clone();
                    config.set("group.id", &reply_topic).set("enable.auto.commit", "true").set("auto.offset.reset", "earliest");
                    let replies: StreamConsumer = config.create()?;
                    replies.subscribe(&[reply_topic.as_str()])?;
                    Ok::<_, BoxError>((replies, producer(&base)?))
                })
                .await??
        };

        let (closed, closing) = oneshot::channel();
        let side = Arc::new(ClientSide {
            producer: Mutex::new(Some(producer)),
            codec: self.codec,
            id,
            reply_topic,
            calls: Mutex::new(HashMap::new()),
            closed: Mutex::new(Some(closed)),
            routing: Mutex::new(None),
        });
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        runtime.handle().spawn(client_writer(Arc::clone(&side), queued));
        let (frames, replies_out) = mpsc::unbounded_channel();
        let routing = runtime.handle().spawn(route_replies(Arc::clone(&side), replies, frames, closing, writes.clone()));
        *lock(&side.routing) = Some(routing);
        lock(&self.state).client = Some(Arc::clone(&side));

        Ok(Outbound {
            // Queued at the call, so frames go out in the order their sends were called.
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let (answer, answered) = oneshot::channel();
                let room = writes.send(Job::Send { pattern, frame, answer });
                Box::pin(async move {
                    room.await.map_err(|_| BoxError::from(CLOSED))?;
                    answered.await.map_err(|_| BoxError::from(CLOSED))?
                })
            }),
            replies: receiver_stream(replies_out),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.clone();
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        pause(&side.consumer);
        if let Err(error) = side.consumer.commit_consumer_state(CommitMode::Async) {
            tracing::debug!(%error, "the Kafka link had no offset to commit at the drain");
        }
        let watched = Arc::clone(&side);
        side.runtime.handle().spawn(async move {
            let mut count = watched.calls.count.subscribe();
            let _ = count.wait_for(|held| *held == 0).await;
            lock(&watched.deliveries).take();
        });
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (server, client) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take())
        };
        if let Some(side) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            lock(&side.deliveries).take();
            // A synchronous commit blocks its thread until the broker answers, or until
            // librdkafka's request timeout once the broker is gone, so it runs on one of its own.
            let consumer = Arc::clone(&side.consumer);
            match blocking(move || consumer.commit_consumer_state(CommitMode::Sync)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::debug!(%error, "the Kafka link had no offset to commit at close"),
                Err(error) => tracing::warn!(%error, "the Kafka link's commit at close failed"),
            }
        }
        if let Some(side) = client {
            side.close().await;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// The broker list as librdkafka takes it, and whether it asked for SSL.
struct Brokers {
    list: String,
    ssl: bool,
}

impl Brokers {
    fn parse(text: &str) -> Result<Brokers, BoxError> {
        let mut entries = Vec::new();
        let mut ssl = None;
        for entry in text.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
            let lower = entry.to_ascii_lowercase();
            let (secure, address) = if lower.starts_with("ssl://") {
                (true, &entry["ssl://".len()..])
            } else if lower.starts_with("plaintext://") {
                (false, &entry["plaintext://".len()..])
            } else {
                (false, entry)
            };
            if *ssl.get_or_insert(secure) != secure {
                return Err("the Kafka link's brokers mix `ssl://` and plaintext entries".into());
            }
            let port = address.rsplit_once(':').map(|(_, port)| port);
            if port.is_none_or(|port| port.parse::<u16>().is_err()) {
                return Err(format!("the Kafka link's broker `{entry}` is not `host:port`").into());
            }
            entries.push(address.to_owned());
        }
        if entries.is_empty() {
            return Err("the Kafka link names no broker".into());
        }
        let ssl = ssl.unwrap_or(false);
        if ssl && !cfg!(feature = "tls") {
            return Err("the Kafka link's brokers ask for `ssl://`, which needs the crate's `tls` feature".into());
        }
        Ok(Brokers { list: entries.join(","), ssl })
    }

    fn config(&self) -> ClientConfig {
        let mut config = ClientConfig::new();
        config.set("bootstrap.servers", &self.list);
        if self.ssl {
            config.set("security.protocol", "ssl");
        }
        config
    }
}

/// Creates `topics`, an existing one being no failure.
async fn create_topics(base: &ClientConfig, topics: &[String], partitions: i32, replication: i32) -> Result<(), BoxError> {
    let admin: AdminClient<DefaultClientContext> = base.create()?;
    let new: Vec<NewTopic<'_>> =
        topics.iter().map(|topic| NewTopic::new(topic, partitions, TopicReplication::Fixed(replication))).collect();
    for result in admin.create_topics(&new, &AdminOptions::new()).await? {
        if let Err((topic, code)) = result
            && code != RDKafkaErrorCode::TopicAlreadyExists
        {
            return Err(format!("the Kafka link could not create the topic `{topic}`: {code}").into());
        }
    }
    Ok(())
}

/// Commits the end offset of each of `topics`' partitions on which `group` has none committed, so
/// a partition the group has never consumed starts where the group first bound. Left at
/// `auto.offset.reset`, such a partition would start at its end each time it is assigned, and a
/// record produced while no instance consumed it, or while a rebalance moved it, would be skipped.
/// Kafka accepts a commit from outside a group only while the group is empty, so an instance
/// joining a running group leaves the offsets to the instances already in it.
///
/// Reading the committed offsets asks the group's coordinator, which a broker that has just
/// started, or is moving the coordinator, may not have ready: those three errors are retried
/// within the call's own timeout (`coordinator_retried`), waiting on the app's `timer` between
/// attempts; any other fails `bind` at once. Each broker call blocks its thread and runs on one
/// of its own ([`blocking`]).
async fn anchor(base: &ClientConfig, group: &str, topics: &[String], timer: &Arc<dyn Timer>) -> Result<(), BoxError> {
    let mut config = base.clone();
    config.set("group.id", group).set("enable.auto.commit", "false");
    let consumer: Arc<Detached<BaseConsumer>> = Arc::new(Detached::new(config.create()?));
    let timeout = Timeout::After(ANCHOR_TIMEOUT);
    let reader = Arc::clone(&consumer);
    let topics = topics.to_vec();
    let every = blocking(move || -> Result<TopicPartitionList, BoxError> {
        let mut every = TopicPartitionList::new();
        for topic in &topics {
            let metadata = reader.fetch_metadata(Some(topic), timeout)?;
            for partition in metadata.topics().iter().filter(|found| found.name() == topic).flat_map(|found| found.partitions()) {
                every.add_partition(topic, partition.id());
            }
        }
        Ok(every)
    })
    .await??;
    let deadline = timer.now() + ANCHOR_TIMEOUT;
    let committed = coordinator_retried(
        deadline,
        |remaining| {
            let reader = Arc::clone(&consumer);
            let every = every.clone();
            async move { blocking(move || reader.committed_offsets(every, Timeout::After(remaining)).map_err(BoxError::from)).await? }
        },
        |backoff| timer.sleep(backoff),
        || timer.now(),
    )
    .await?;
    let group = group.to_owned();
    blocking(move || -> Result<(), BoxError> {
        let mut start = TopicPartitionList::new();
        for unset in committed.elements().into_iter().filter(|element| element.offset() == Offset::Invalid) {
            let (_, end) = consumer.fetch_watermarks(unset.topic(), unset.partition(), timeout)?;
            start.add_partition_offset(unset.topic(), unset.partition(), Offset::Offset(end))?;
        }
        if start.count() > 0
            && let Err(error) = consumer.commit(&start, CommitMode::Sync)
        {
            tracing::debug!(%error, group, "the Kafka link left the starting offsets to the group's running instances");
        }
        Ok(())
    })
    .await?
}

/// How long each of `anchor`'s broker calls may take, the committed-offsets read with its
/// retries included.
const ANCHOR_TIMEOUT: Duration = Duration::from_secs(10);

/// The first wait before a retry of a coordinator not yet ready, doubled after each up to
/// [`RETRY_BACKOFF_MAX`].
const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// `attempt`, given the time left before `deadline`, retried while it fails with one of the
/// three errors a group coordinator answers while it loads or moves, which librdkafka classes
/// retriable, and a retry still fits before `deadline`; the last error is returned once one does
/// not. Any other error is returned at once. `sleep` and `now` are the clock, the app's `Timer`
/// in `anchor`.
async fn coordinator_retried<T, A, S>(
    deadline: Instant,
    mut attempt: impl FnMut(Duration) -> A,
    mut sleep: impl FnMut(Duration) -> S,
    now: impl Fn() -> Instant,
) -> Result<T, BoxError>
where
    A: Future<Output = Result<T, BoxError>>,
    S: Future<Output = ()>,
{
    let mut backoff = RETRY_BACKOFF;
    loop {
        let outcome = attempt(deadline.saturating_duration_since(now())).await;
        match &outcome {
            Err(error) if coordinator_not_ready(error) && now() + backoff < deadline => {
                tracing::debug!(%error, ?backoff, "the Kafka group coordinator is not ready; retrying");
                sleep(backoff).await;
                backoff = (backoff * 2).min(RETRY_BACKOFF_MAX);
            }
            _ => return outcome,
        }
    }
}

fn coordinator_not_ready(error: &BoxError) -> bool {
    matches!(
        error.downcast_ref::<KafkaError>(),
        Some(KafkaError::MetadataFetch(
            RDKafkaErrorCode::NotCoordinator | RDKafkaErrorCode::CoordinatorLoadInProgress | RDKafkaErrorCode::CoordinatorNotAvailable
        ))
    )
}

/// Runs `call` on a thread of its own and answers its result: a librdkafka call that blocks its
/// thread, kept off the runtime's workers as a consumer's drop is ([`Detached`]). Not a
/// `spawn_blocking` task, which needs tokio's runtime and which a runtime waits for as it shuts
/// down. A thread that cannot be spawned runs `call` here; a call that panics is an error.
async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> Result<T, BoxError> {
    let (done, answered) = oneshot::channel();
    let call = Arc::new(Mutex::new(Some(call)));
    let on_thread = Arc::clone(&call);
    let spawned = std::thread::Builder::new().name("ulo-kafka-blocking".to_owned()).spawn(move || {
        if let Some(call) = lock(&on_thread).take() {
            let _ = done.send(call());
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the Kafka link could not spawn a thread for a blocking call, and made it here");
        let call = lock(&call).take().ok_or("the Kafka link's blocking call was taken twice")?;
        return Ok(call());
    }
    answered.await.map_err(|_| BoxError::from("the Kafka link's blocking call panicked"))
}

fn producer(base: &ClientConfig) -> Result<FutureProducer, BoxError> {
    let mut config = base.clone();
    config.set("message.max.bytes", MAX_MESSAGE.to_string()).set("message.timeout.ms", DELIVERY_TIMEOUT);
    Ok(config.create()?)
}

/// A consumer of the control topic assigned every partition from its end, outside any group's
/// rebalancing, so it reads from `bind` on rather than from a first assignment seconds later. Its
/// `group.id` is this instance's own and it commits nothing.
async fn control_consumer(base: &ClientConfig, group: &str) -> Result<Arc<Detached<StreamConsumer>>, BoxError> {
    let mut config = base.clone();
    config
        .set("group.id", format!("{group}.control.{}", uuid::Uuid::new_v4().simple()))
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "latest");
    let consumer: Arc<Detached<StreamConsumer>> = Arc::new(Detached::new(config.create()?));
    let reader = Arc::clone(&consumer);
    // `fetch_metadata` blocks the calling thread.
    let partitions: Vec<i32> = blocking(move || -> Result<Vec<i32>, BoxError> {
        let metadata = reader.fetch_metadata(Some(CONTROL), Timeout::After(Duration::from_secs(10)))?;
        Ok(metadata
            .topics()
            .iter()
            .filter(|topic| topic.name() == CONTROL)
            .flat_map(|topic| topic.partitions().iter().map(|partition| partition.id()))
            .collect())
    })
    .await??;
    if partitions.is_empty() {
        return Err(format!("the Kafka link found no partition of `{CONTROL}`").into());
    }
    let mut assignment = TopicPartitionList::new();
    for partition in partitions {
        assignment.add_partition_offset(CONTROL, partition, Offset::End)?;
    }
    consumer.assign(&assignment)?;
    Ok(consumer)
}

/// The server side of a bound link: the group's consumer, the reply producer, the calls it holds,
/// and where deliveries go.
struct ServerSide {
    consumer: Arc<Detached<StreamConsumer>>,
    /// The server's writer, which every reply and `opened` is produced through.
    writes: ordered::Sender<Record>,
    codec: Codec,
    /// The link's runtime, which the drain's watcher is spawned on.
    runtime: Tokio,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    /// The records of the group's topics handed over and not yet settled: each is counted until
    /// its `Ack` settles or is dropped, which the server does only after freeing the call's place.
    inflight: Arc<watch::Sender<usize>>,
    /// `Link::max_inflight`'s bound, at which the request lane pauses the assignment.
    limit: Option<usize>,
}

impl ServerSide {
    fn deliver(&self, delivery: Delivery) {
        if let Some(deliveries) = lock(&self.deliveries).as_ref() {
            let _ = deliveries.send(delivery);
        }
    }

    async fn on_request(&self, record: OwnedMessage) {
        let pattern = record.topic().to_owned();
        let headers = record.headers();
        let kind = header(headers, KIND);
        let reply = header(headers, REPLY_TO);
        let correlation = header(headers, CORRELATION);
        let call_headers = call_headers(headers);
        let data = Data::new(record.payload().map(Bytes::copy_from_slice).unwrap_or_default());
        let consumer = Arc::clone(&self.consumer);
        let (partition, offset) = (record.partition(), record.offset());
        let topic = pattern.clone();
        // Stored once the handler completes and committed by the next auto-commit, for a
        // rejected event too, so it cannot loop on redelivery. Unsettled, a restarted group
        // reads the record again. Counted in flight until settled or dropped.
        let counted = InFlight::new(Arc::clone(&self.inflight));
        let ack = Ack::new(move |_accepted| {
            if let Err(error) = consumer.store_offset(&topic, partition, offset) {
                tracing::warn!(%error, topic, "the Kafka link could not store an offset");
            }
            drop(counted);
        });
        match (reply, kind.as_deref()) {
            (None, None) => self.deliver(Delivery { frame: Frame::Evt { pattern, headers: call_headers, data }, reply: None, ack }),
            (Some(reply), None) => {
                let key = correlation.clone().unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply, correlation);
                self.deliver(Delivery { frame: Frame::Req { id, pattern, headers: call_headers, data }, reply: Some(path), ack });
            }
            (Some(reply), Some(OPEN)) => {
                let key = correlation.clone().unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply.clone(), correlation.clone());
                // The caller holds the request's items until this arrives, so they reach the
                // control topic after the call is held here.
                if let Err(error) = write(&self.writes, reply.clone(), correlation.clone(), Some(OPENED), Bytes::new()).await {
                    tracing::warn!(%error, pattern, "the Kafka link could not acknowledge a streamed request");
                }
                self.deliver(Delivery { frame: Frame::Open { id, pattern, headers: call_headers }, reply: Some(path), ack });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern, kind, "the Kafka link dropped a record of an unknown kind");
                ack.reject();
            }
        }
    }

    fn on_control(&self, record: OwnedMessage) {
        let headers = record.headers();
        let Some(key) = header(headers, CORRELATION) else { return };
        let frame = match header(headers, KIND).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| {
                let data = Data::new(record.payload().map(Bytes::copy_from_slice).unwrap_or_default());
                (Frame::In { id, data }, path)
            }),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under `key`, its correlation id, the key its control records carry.
    fn hold(&self, key: String, reply: String, correlation: Option<String>) -> (u64, ReplyPath) {
        let writes = self.writes.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let released = key.clone();
        // Queued at the call, so replies go out in the order their sends were called.
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            if is_terminal(&frame) {
                calls.release(&released);
            }
            let written = codec.encode_frame(&frame).map(|bytes| write(&writes, reply.clone(), correlation.clone(), None, bytes));
            Box::pin(async move { written?.await })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

/// The group consumer's records until the drain, which pauses the partitions and leaves the
/// consumer to the acknowledgments still owed. At the server's in-flight bound the lane pauses
/// the assignment until a record settles ([`at_bound`]), so the requests over it wait in their
/// topics.
async fn request_lane(side: Arc<ServerSide>) {
    let mut phase = side.phase.subscribe();
    let consumer = Arc::clone(&side.consumer);
    loop {
        // The borrowed record is detached inside the arm: it borrows the consumer and is not
        // `Send`, so it must not live across an await.
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() != Phase::Serving {
                    break;
                }
                continue;
            }
            record = consumer.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => {
                side.on_request(record).await;
                if !at_bound(&side, &mut phase).await {
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Holds the request lane while the records in flight reach the server's bound: the assignment
/// paused, then resumed once one settles. `pause` is local to librdkafka and drops the records it
/// fetched ahead, so the paused partitions resume from the next record the lane has not read. The
/// consumer is still read meanwhile, which serves a rebalance; a record from a partition assigned
/// since the pause is kept here, the new assignment paused in turn, and handed over once there is
/// room. `false` once the server drains or closes, the partitions left paused for the drain.
async fn at_bound(side: &ServerSide, phase: &mut watch::Receiver<Phase>) -> bool {
    let Some(limit) = side.limit else { return true };
    if *side.inflight.borrow() < limit {
        return true;
    }
    pause(&side.consumer);
    let mut inflight = side.inflight.subscribe();
    let mut kept = VecDeque::new();
    loop {
        if *inflight.borrow_and_update() < limit {
            match kept.pop_front() {
                Some(record) => {
                    side.on_request(record).await;
                    continue;
                }
                None => break,
            }
        }
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() != Phase::Serving {
                    return false;
                }
                continue;
            }
            _ = inflight.changed() => continue,
            record = side.consumer.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => {
                pause(&side.consumer);
                kept.push_back(record);
            }
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's consumer failed a read while paused; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    if *phase.borrow() == Phase::Serving {
        match side.consumer.assignment() {
            Ok(assignment) => {
                if let Err(error) = side.consumer.resume(&assignment) {
                    tracing::warn!(%error, "the Kafka link could not resume its partitions");
                }
            }
            Err(error) => tracing::warn!(%error, "the Kafka link could not read its assignment"),
        }
    }
    true
}

/// Pauses every partition currently assigned to `consumer`.
fn pause(consumer: &StreamConsumer) {
    match consumer.assignment() {
        Ok(assignment) => {
            if let Err(error) = consumer.pause(&assignment) {
                tracing::warn!(%error, "the Kafka link could not pause its partitions");
            }
        }
        Err(error) => tracing::warn!(%error, "the Kafka link could not read its assignment"),
    }
}

/// One record counted in [`ServerSide::inflight`] until dropped.
struct InFlight(Arc<watch::Sender<usize>>);

impl InFlight {
    fn new(count: Arc<watch::Sender<usize>>) -> Self {
        count.send_modify(|count| *count += 1);
        InFlight(count)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

async fn control_lane(side: Arc<ServerSide>, control: Arc<Detached<StreamConsumer>>) {
    let mut phase = side.phase.subscribe();
    loop {
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() == Phase::Closed {
                    break;
                }
                continue;
            }
            record = control.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => side.on_control(record),
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's control consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// The calls a server holds, by the key their control records carry, each with its local id and
/// its reply path. The local id is this link's own, unique across every caller.
struct Calls {
    next: AtomicU64,
    held: Mutex<HashMap<String, (u64, ReplyPath)>>,
    count: watch::Sender<usize>,
}

impl Calls {
    fn new() -> Self {
        Calls { next: AtomicU64::new(1), held: Mutex::new(HashMap::new()), count: watch::channel(0).0 }
    }

    fn hold(&self, key: String, path: ReplyPath) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut held = lock(&self.held);
        held.insert(key, (id, path));
        self.count.send_replace(held.len());
        id
    }

    fn get(&self, key: &str) -> Option<(u64, ReplyPath)> {
        lock(&self.held).get(key).cloned()
    }

    fn release(&self, key: &str) -> Option<(u64, ReplyPath)> {
        let mut held = lock(&self.held);
        let call = held.remove(key);
        self.count.send_replace(held.len());
        call
    }

    /// Drops every held path, each of which holds this table through its closure.
    fn clear(&self) {
        lock(&self.held).clear();
        self.count.send_replace(0);
    }
}

/// One reply-lane record for the server's writer, its delivery's outcome answered on `answer`.
struct Record {
    topic: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
    answer: oneshot::Sender<Result<(), BoxError>>,
}

/// The delivery reports a writer waits on while it queues the next records.
type Delivering = FuturesUnordered<BoxFuture<'static, ()>>;

/// Queues the server's replies with librdkafka one after another, in the order they were queued,
/// waiting for each delivery report beside the records that follow, until every sender is gone.
async fn server_writer(producer: FutureProducer, mut queued: ordered::Receiver<Record>) {
    let mut delivering = Delivering::new();
    loop {
        let Record { topic, correlation, kind, body, answer } = tokio::select! {
            record = queued.recv() => match record {
                Some(record) => record,
                None => break,
            },
            Some(()) = delivering.next(), if !delivering.is_empty() => continue,
        };
        match produce(&producer, &topic, correlation.as_deref(), kind, &CallHeaders::new(), &body).await {
            Ok(delivery) => delivering.push(Box::pin(async move {
                let _ = answer.send(delivery.await);
            })),
            Err(error) => {
                let _ = answer.send(Err(error));
            }
        }
    }
    while delivering.next().await.is_some() {}
}

/// Queues one message on the server's writer at the call and answers a wait for it to be
/// published.
fn write(
    writes: &ordered::Sender<Record>,
    topic: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
    let (answer, answered) = oneshot::channel();
    let room = writes.send(Record { topic, correlation, kind, body, answer });
    async move {
        let gone = || BoxError::from("the Kafka link's server side is closed");
        room.await.map_err(|_| gone())?;
        answered.await.map_err(|_| gone())?
    }
}

/// The client side: the producer, the reply topic every reply lands on, each call's correlation
/// id `<id>.<call>`.
struct ClientSide {
    /// `None` once the link has closed.
    producer: Mutex<Option<FutureProducer>>,
    codec: Codec,
    id: String,
    reply_topic: String,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Stops the reply router at close.
    closed: Mutex<Option<oneshot::Sender<()>>>,
    /// The reply router, which `close` awaits: it drops the reply consumer as it ends.
    routing: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

enum ClientCall {
    Unary,
    /// A streamed request: until the server acknowledges the `open`, its `in` and `in_end` frames
    /// wait in `held`, and the writer produces them in order once it does.
    Streaming { opened: bool, held: VecDeque<Frame> },
    /// A streamed request cancelled before the server acknowledged its `open`: the `cancel` waits
    /// for the acknowledgment and is produced alone, the frames held before it dropped. The entry stays
    /// until then, or until the link closes when no acknowledgment comes.
    Cancelled,
}

const CLOSED: &str = "the Kafka link's client is closed";

/// What the client's writer takes, in order: a frame to send, or the server's `opened` for a
/// streamed request, which releases the frames held for it.
enum Job {
    Send { pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>> },
    Opened(u64),
}

/// Queues the client's records with librdkafka in the order they were queued, waiting for each
/// delivery report beside the records that follow, until every sender is gone.
async fn client_writer(side: Arc<ClientSide>, mut queued: ordered::Receiver<Job>) {
    let mut delivering = Delivering::new();
    loop {
        let job = tokio::select! {
            job = queued.recv() => match job {
                Some(job) => job,
                None => break,
            },
            Some(()) = delivering.next(), if !delivering.is_empty() => continue,
        };
        match job {
            Job::Send { pattern, frame, answer } => side.send(pattern, frame, answer, &mut delivering).await,
            Job::Opened(call) => {
                for frame in side.opened(call) {
                    side.produce_held(call, frame, &mut delivering).await;
                }
            }
        }
    }
    while delivering.next().await.is_some() {}
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>>, delivering: &mut Delivering) {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                lock(&self.calls).insert(id, ClientCall::Unary);
                let queued = self.produce_request(&pattern, id, &headers, None, data.as_bytes()).await;
                self.settle(queued, Some(id), answer, delivering);
            }
            Frame::Evt { headers, data, .. } => {
                let record_headers = append(OwnedHeaders::new(), &headers);
                let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(data.as_bytes()).headers(record_headers);
                let queued = match self.live_producer() {
                    Ok(producer) => enqueue(&producer, record, pattern.as_str(), data.len()).await,
                    Err(error) => Err(error),
                };
                self.settle(queued, None, answer, delivering);
            }
            Frame::Open { id, headers, .. } => {
                lock(&self.calls).insert(id, ClientCall::Streaming { opened: false, held: VecDeque::new() });
                let queued = self.produce_request(&pattern, id, &headers, Some(OPEN), &[]).await;
                self.settle(queued, Some(id), answer, delivering);
            }
            Frame::In { id, data } => {
                if data.len() as u64 > MAX_MESSAGE {
                    let _ = answer.send(Err(Box::new(FrameTooLarge { size: data.len() as u64, limit: MAX_MESSAGE })));
                    return;
                }
                self.control(id, Frame::In { id, data }, delivering).await;
                let _ = answer.send(Ok(()));
            }
            Frame::InEnd { id } => {
                self.control(id, Frame::InEnd { id }, delivering).await;
                let _ = answer.send(Ok(()));
            }
            Frame::Cancel { id } => {
                let call = lock(&self.calls).remove(&id);
                match call {
                    Some(ClientCall::Unary) => {
                        let queued = self.produce_control(id, CANCEL, &[]).await;
                        self.settle(queued, None, answer, delivering);
                    }
                    Some(ClientCall::Streaming { opened: true, .. }) => {
                        self.produce_held(id, Frame::Cancel { id }, delivering).await;
                        let _ = answer.send(Ok(()));
                    }
                        // The server takes the `open` and holds the call, so the `cancel` follows once it
                    // acknowledges it.
                    Some(ClientCall::Streaming { opened: false, .. }) => {
                        lock(&self.calls).insert(id, ClientCall::Cancelled);
                        let _ = answer.send(Ok(()));
                    }
                    Some(ClientCall::Cancelled) | None => {
                        let _ = answer.send(Ok(()));
                    }
                }
            }
            other => {
                let _ = answer.send(Err(format!("a client does not send a `{}` frame", other.kind()).into()));
            }
        }
    }

    /// Answers a produce once its delivery report arrives; a request that failed is forgotten.
    fn settle(self: &Arc<Self>, queued: Result<Report, BoxError>, call: Option<u64>, answer: oneshot::Sender<Result<(), BoxError>>, delivering: &mut Delivering) {
        let side = Arc::clone(self);
        let delivery = match queued {
            Ok(delivery) => delivery,
            Err(error) => {
                if let Some(call) = call {
                    lock(&side.calls).remove(&call);
                }
                let _ = answer.send(Err(error));
                return;
            }
        };
        delivering.push(Box::pin(async move {
            let outcome = delivery.await;
            if outcome.is_err()
                && let Some(call) = call
            {
                lock(&side.calls).remove(&call);
            }
            let _ = answer.send(outcome);
        }));
    }

    /// A streamed request's `in` or `in_end`: produced once the server has acknowledged the
    /// `open`, held until then, and dropped for a call that has ended.
    async fn control(&self, id: u64, frame: Frame, delivering: &mut Delivering) {
        let held = {
            let mut calls = lock(&self.calls);
            match calls.get_mut(&id) {
                Some(ClientCall::Streaming { opened: false, held }) => {
                    held.push_back(frame);
                    return;
                }
                Some(ClientCall::Streaming { opened: true, .. }) => frame,
                _ => return,
            }
        };
        self.produce_held(id, held, delivering).await;
    }

    /// The frames held for a streamed request the server has now acknowledged, which the writer
    /// produces before anything queued after the acknowledgment.
    fn opened(&self, id: u64) -> VecDeque<Frame> {
        let mut calls = lock(&self.calls);
        match calls.get_mut(&id) {
            Some(ClientCall::Streaming { opened, held }) if !*opened => {
                *opened = true;
                std::mem::take(held)
            }
            Some(ClientCall::Cancelled) => {
                calls.remove(&id);
                VecDeque::from([Frame::Cancel { id }])
            }
            _ => VecDeque::new(),
        }
    }

    /// Produces one control frame of a streamed request; a failure is logged, as the call's caller
    /// has gone on.
    async fn produce_held(&self, call: u64, frame: Frame, delivering: &mut Delivering) {
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => return,
        };
        match self.produce_control(call, kind, &body).await {
            Ok(delivery) => delivering.push(Box::pin(async move {
                if let Err(error) = delivery.await {
                    tracing::debug!(%error, kind, "the Kafka link could not publish a control frame");
                }
            })),
            Err(error) => tracing::debug!(%error, kind, "the Kafka link could not publish a control frame"),
        }
    }

    fn correlation(&self, call: u64) -> String {
        format!("{}.{call}", self.id)
    }

    fn call_of(&self, correlation: &str) -> Option<u64> {
        correlation.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    /// Keyed by this client's id, so one caller's requests share a partition and stay ordered.
    async fn produce_request(&self, pattern: &Pattern, call: u64, headers: &CallHeaders, kind: Option<&str>, body: &[u8]) -> Result<Report, BoxError> {
        let correlation = self.correlation(call);
        let mut record_headers = OwnedHeaders::new()
            .insert(Header { key: REPLY_TO, value: Some(self.reply_topic.as_str()) })
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) });
        if let Some(kind) = kind {
            record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
        }
        record_headers = append(record_headers, headers);
        let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(body).headers(record_headers);
        enqueue(&self.live_producer()?, record, pattern.as_str(), body.len()).await
    }

    async fn produce_control(&self, call: u64, kind: &str, body: &[u8]) -> Result<Report, BoxError> {
        let correlation = self.correlation(call);
        let record_headers = OwnedHeaders::new()
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) })
            .insert(Header { key: KIND, value: Some(kind) });
        let record = FutureRecord::to(CONTROL).key(correlation.as_str()).payload(body).headers(record_headers);
        enqueue(&self.live_producer()?, record, CONTROL, body.len()).await
    }

    fn live_producer(&self) -> Result<FutureProducer, BoxError> {
        lock(&self.producer).clone().ok_or_else(|| CLOSED.into())
    }

    /// Stops the reply router and waits for it to end, the reply consumer leaving its group while
    /// the broker is still there to answer, then drops the producer, closing its connections. The
    /// `RpcClient` keeps the connection's send half until its next call replaces it, so the
    /// producer is taken out of it here.
    async fn close(&self) {
        if let Some(closed) = lock(&self.closed).take() {
            let _ = closed.send(());
        }
        let routing = lock(&self.routing).take();
        if let Some(routing) = routing {
            let _ = routing.await;
        }
        let producer = lock(&self.producer).take();
        if let Some(producer) = producer {
            let _ = drop_detached(producer).await;
        }
    }
}

async fn route_replies(
    side: Arc<ClientSide>,
    replies: StreamConsumer,
    frames: mpsc::UnboundedSender<Frame>,
    mut closing: oneshot::Receiver<()>,
    writes: ordered::Sender<Job>,
) {
    // Held detached, so a router dropped before it ends, its client never closed and the runtime
    // shutting down, still drops the consumer on a thread of its own.
    let replies = Detached::new(replies);
    loop {
        let record = tokio::select! {
            _ = &mut closing => break,
            record = replies.recv() => record.map(|record| record.detach()),
        };
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's reply consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let headers = record.headers();
        let Some(call) = header(headers, CORRELATION).and_then(|correlation| side.call_of(&correlation)) else { continue };
        if header(headers, KIND).as_deref() == Some(OPENED) {
            // To the writer, behind the frames already queued.
            if writes.send(Job::Opened(call)).await.is_err() {
                break;
            }
            continue;
        }
        let frame = match side.codec.decode_frame(record.payload().unwrap_or_default()) {
            Ok(frame) => with_id(frame, call),
            Err(error) => {
                tracing::warn!(%error, "the Kafka link dropped a reply that does not decode");
                continue;
            }
        };
        if is_terminal(&frame) {
            lock(&side.calls).remove(&call);
        }
        if frames.send(frame).is_err() {
            break;
        }
    }
    let _ = drop_detached(replies.into_inner()).await;
}

/// A record's delivery report, once librdkafka has queued the record.
type Report = BoxFuture<'static, Result<(), BoxError>>;

/// One reply-lane record, keyed by the call's correlation id so one call's frames share a
/// partition and stay ordered.
async fn produce(
    producer: &FutureProducer,
    topic: &str,
    correlation: Option<&str>,
    kind: Option<&str>,
    headers: &CallHeaders,
    body: &[u8],
) -> Result<Report, BoxError> {
    let mut record_headers = OwnedHeaders::new();
    if let Some(correlation) = correlation {
        record_headers = record_headers.insert(Header { key: CORRELATION, value: Some(correlation) });
    }
    if let Some(kind) = kind {
        record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
    }
    record_headers = append(record_headers, headers);
    let record = FutureRecord::to(topic).key(correlation.unwrap_or_default()).payload(body).headers(record_headers);
    enqueue(producer, record, topic, body.len()).await
}

/// Queues `record` with librdkafka, waiting up to five seconds for room in its queue, and answers
/// the record's delivery report. An unknown topic is the miss signal where the broker does not
/// auto-create topics; an oversized record is `FrameTooLarge`. The queueing is the writer's, in
/// order; the report is waited on beside the records queued after it.
async fn enqueue(producer: &FutureProducer, record: FutureRecord<'_, str, [u8]>, topic: &str, size: usize) -> Result<Report, BoxError> {
    if size as u64 > MAX_MESSAGE {
        return Err(Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }));
    }
    let started = Instant::now();
    let mut record = record;
    loop {
        match producer.send_result(record) {
            Ok(report) => {
                let topic = topic.to_owned();
                return Ok(Box::pin(async move {
                    match report.await {
                        Ok(Ok(_)) => Ok(()),
                        Ok(Err((error, _))) => Err(refused(error, &topic, size)),
                        Err(_) => Err("the Kafka producer was dropped before the record's delivery report".into()),
                    }
                }));
            }
            Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), back)) if started.elapsed() < QUEUE_TIMEOUT => {
                record = back;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err((error, _)) => return Err(refused(error, topic, size)),
        }
    }
}

/// A produce librdkafka refused, mapped as `RpcClient` reads a link's refusal.
fn refused(error: KafkaError, topic: &str, size: usize) -> BoxError {
    match error {
        KafkaError::MessageProduction(RDKafkaErrorCode::UnknownTopicOrPartition | RDKafkaErrorCode::UnknownTopic) => {
            Box::new(NoDestination { pattern: topic.to_owned() })
        }
        KafkaError::MessageProduction(RDKafkaErrorCode::MessageSizeTooLarge) => Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }),
        error => Box::new(error),
    }
}

fn append(mut record_headers: OwnedHeaders, headers: &CallHeaders) -> OwnedHeaders {
    for (name, value) in headers.iter() {
        record_headers = record_headers.insert(Header { key: name, value: Some(value) });
    }
    record_headers
}

fn header<H: Headers>(headers: Option<&H>, name: &str) -> Option<String> {
    let headers = headers?;
    (0..headers.count())
        .map(|index| headers.get(index))
        .find(|header| header.key == name)
        .and_then(|header| header.value)
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

/// The call's headers from the record's, the link's reserved names left out.
fn call_headers<H: Headers>(headers: Option<&H>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for index in 0..headers.count() {
            let header = headers.get(index);
            if matches!(header.key, KIND | REPLY_TO | CORRELATION) {
                continue;
            }
            if let Some(value) = header.value {
                out.insert(header.key, String::from_utf8_lossy(value).into_owned());
            }
        }
    }
    out
}

/// The reply frame with the caller's own `id`, read from the correlation id: the server wrote its
/// local id.
fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

fn is_terminal(frame: &Frame) -> bool {
    matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. })
}

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

/// A value whose drop runs on a thread of its own, for a librdkafka consumer: its drop leaves the
/// group and polls until librdkafka confirms, about a tenth of a second with the broker up and
/// without bound once the broker is gone, and on a runtime worker that stalls every task
/// scheduled there.
struct Detached<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> Detached<T> {
    fn new(value: T) -> Self {
        Detached(Some(value))
    }

    /// The value, for a caller that drops it with [`drop_detached`] and waits for the drop.
    fn into_inner(mut self) -> T {
        self.0.take().expect("the value is taken only once")
    }
}

impl<T: Send + 'static> Deref for Detached<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().expect("the value is taken only by the drop")
    }
}

impl<T: Send + 'static> Drop for Detached<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            let _ = drop_detached(value);
        }
    }
}

/// Drops `value` on a thread of its own and answers once the drop has finished: a consumer, or the
/// client's producer, whose drop flushes for up to half a second before closing its connections.
/// Not a `spawn_blocking` task: a runtime waits for those as it shuts down, and a consumer whose
/// broker is gone would hold it there. A thread that cannot be spawned drops `value` here.
fn drop_detached<T: Send + 'static>(value: T) -> oneshot::Receiver<()> {
    let (done, finished) = oneshot::channel();
    let spawned = std::thread::Builder::new().name("ulo-kafka-drop".to_owned()).spawn(move || {
        drop(value);
        let _ = done.send(());
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the Kafka link could not spawn a thread to drop a librdkafka handle on, and dropped it here");
    }
    finished
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Ready, ready};

    use futures_util::FutureExt;

    use super::*;

    /// A clock that `sleep` advances, starting at `start`.
    struct Clock {
        now: Cell<Instant>,
        slept: RefCell<Vec<Duration>>,
    }

    impl Clock {
        fn new(start: Instant) -> Self {
            Clock { now: Cell::new(start), slept: RefCell::new(Vec::new()) }
        }

        fn sleep(&self, d: Duration) -> Ready<()> {
            self.now.set(self.now.get() + d);
            self.slept.borrow_mut().push(d);
            ready(())
        }
    }

    fn failing(code: RDKafkaErrorCode) -> BoxError {
        BoxError::from(KafkaError::MetadataFetch(code))
    }

    /// `coordinator_retried` run to its end: every attempt and sleep here is ready at once.
    fn retried<T>(
        deadline: Instant,
        mut attempt: impl FnMut(Duration) -> Result<T, BoxError>,
        clock: &Clock,
    ) -> Result<T, BoxError> {
        coordinator_retried(deadline, |left| ready(attempt(left)), |d| clock.sleep(d), || clock.now.get())
            .now_or_never()
            .expect("every attempt and sleep is ready at once")
    }

    fn code_of(outcome: &Result<(), BoxError>) -> Option<RDKafkaErrorCode> {
        match outcome.as_ref().err().and_then(|error| error.downcast_ref::<KafkaError>()) {
            Some(KafkaError::MetadataFetch(code)) => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn each_coordinator_error_is_retried_until_the_read_succeeds() {
        for code in [
            RDKafkaErrorCode::NotCoordinator,
            RDKafkaErrorCode::CoordinatorLoadInProgress,
            RDKafkaErrorCode::CoordinatorNotAvailable,
        ] {
            let start = Instant::now();
            let clock = Clock::new(start);
            let mut calls = 0;
            let outcome = retried(
                start + ANCHOR_TIMEOUT,
                |_| {
                    calls += 1;
                    if calls < 3 { Err(failing(code)) } else { Ok(calls) }
                },
                &clock,
            );
            assert_eq!(outcome.ok(), Some(3), "{code:?} was not retried until the read succeeded");
            assert_eq!(*clock.slept.borrow(), vec![RETRY_BACKOFF, RETRY_BACKOFF * 2], "{code:?}: the waits between attempts");
        }
    }

    #[test]
    fn any_other_error_fails_at_once() {
        let start = Instant::now();
        let clock = Clock::new(start);
        let mut calls = 0;
        let outcome: Result<(), BoxError> = retried(
            start + ANCHOR_TIMEOUT,
            |_| {
                calls += 1;
                Err(failing(RDKafkaErrorCode::BrokerTransportFailure))
            },
            &clock,
        );
        assert_eq!(code_of(&outcome), Some(RDKafkaErrorCode::BrokerTransportFailure), "{outcome:?}");
        assert_eq!(calls, 1, "an error other than the coordinator's three was retried");
        assert!(clock.slept.borrow().is_empty());
    }

    #[test]
    fn retries_stop_before_the_deadline_and_return_the_last_error() {
        let start = Instant::now();
        let clock = Clock::new(start);
        let mut remaining = Vec::new();
        let outcome: Result<(), BoxError> = retried(
            start + ANCHOR_TIMEOUT,
            |left| {
                remaining.push(left);
                Err(failing(RDKafkaErrorCode::CoordinatorLoadInProgress))
            },
            &clock,
        );
        assert_eq!(code_of(&outcome), Some(RDKafkaErrorCode::CoordinatorLoadInProgress), "{outcome:?}");
        let slept: Duration = clock.slept.borrow().iter().sum();
        assert!(slept < ANCHOR_TIMEOUT, "the retries slept {slept:?}, past the {ANCHOR_TIMEOUT:?} timeout");
        assert_eq!(remaining.first(), Some(&ANCHOR_TIMEOUT), "the first attempt was not given the whole timeout");
        assert!(remaining.windows(2).all(|pair| pair[1] < pair[0]), "each attempt was not given what was left: {remaining:?}");
    }
}
