use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::error::KafkaError;
use rdkafka::message::{Header, Headers, Message, OwnedHeaders, OwnedMessage};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::types::RDKafkaErrorCode;
use rdkafka::util::Timeout;
use rdkafka::{Offset, TopicPartitionList};
use tokio::sync::{mpsc, oneshot, watch};
use ulo::{AppHandle, BoxError, BoxFuture};
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, Frame, FrameTooLarge, Link, NoDestination,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};

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

/// The Kafka link.
pub struct Kafka {
    pub(crate) brokers: String,
    pub(crate) group: Option<String>,
    pub(crate) reply_topic: Option<String>,
    pub(crate) codec: Codec,
    pub(crate) partitions: i32,
    pub(crate) replication: i32,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
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
            group: None,
            reply_topic: None,
            codec: Codec::Json,
            partitions: 1,
            replication: 1,
            default_group: None,
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
}

impl Link for Kafka {
    const NAME: &'static str = "kafka";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerPartition)
            .native_backpressure(true)
            .max_frame(Some(MAX_MESSAGE))
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
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
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the Kafka link's group is unset: `listen` ran before `prepare`")?;
        let base = Brokers::parse(&self.brokers)?.config();

        // Created here, not on the first produce, so a stopped server's topic still exists and a
        // miss is the caller's `Timeout` whether or not the broker auto-creates topics; the
        // consumer also gets its partitions at once rather than after a metadata refresh.
        let topics: Vec<String> = patterns.iter().map(|pattern| pattern.as_str().to_owned()).collect();
        create_topics(&base, &topics, self.partitions, self.replication).await?;
        create_topics(&base, &[CONTROL.to_owned()], 1, self.replication).await?;

        let mut config = base.clone();
        config
            .set("group.id", &group)
            .set("enable.auto.commit", "true")
            .set("enable.auto.offset.store", "false")
            .set("auto.offset.reset", "latest");
        let consumer: Arc<StreamConsumer> = Arc::new(config.create()?);
        let names: Vec<&str> = topics.iter().map(String::as_str).collect();
        consumer.subscribe(&names)?;

        let control = control_consumer(&base, &group).await?;
        let producer = producer(&base)?;

        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let side = Arc::new(ServerSide {
            consumer,
            producer,
            codec: self.codec,
            calls: Arc::new(Calls::new()),
            phase,
            deliveries: Mutex::new(Some(deliveries)),
        });
        tokio::spawn(request_lane(Arc::clone(&side)));
        tokio::spawn(control_lane(Arc::clone(&side), control));
        lock(&self.state).server = Some(side);
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let base = Brokers::parse(&self.brokers)?.config();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let reply_topic = self.reply_topic.clone().unwrap_or_else(|| format!("ulo.rpc.reply.{id}"));
        create_topics(&base, std::slice::from_ref(&reply_topic), self.partitions, self.replication).await?;

        // One group per reply topic reads every partition of it; `earliest` keeps a reply that
        // lands before the group's first assignment.
        let mut config = base.clone();
        config.set("group.id", &reply_topic).set("enable.auto.commit", "true").set("auto.offset.reset", "earliest");
        let replies: StreamConsumer = config.create()?;
        replies.subscribe(&[reply_topic.as_str()])?;

        let (closed, closing) = oneshot::channel();
        let side = Arc::new(ClientSide {
            producer: producer(&base)?,
            codec: self.codec,
            id,
            reply_topic,
            calls: Mutex::new(HashMap::new()),
            closed: Mutex::new(Some(closed)),
        });
        let (frames, replies_out) = mpsc::unbounded_channel();
        tokio::spawn(route_replies(Arc::clone(&side), replies, frames, closing));
        lock(&self.state).client = Some(Arc::clone(&side));

        Ok(Outbound {
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let side = Arc::clone(&side);
                Box::pin(async move { side.send(pattern, frame).await })
            }),
            replies: receiver_stream(replies_out),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.clone();
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        match side.consumer.assignment() {
            Ok(assignment) => {
                if let Err(error) = side.consumer.pause(&assignment) {
                    tracing::warn!(%error, "the Kafka link could not pause its partitions");
                }
            }
            Err(error) => tracing::warn!(%error, "the Kafka link could not read its assignment"),
        }
        if let Err(error) = side.consumer.commit_consumer_state(CommitMode::Async) {
            tracing::debug!(%error, "the Kafka link had no offset to commit at the drain");
        }
        let watched = Arc::clone(&side);
        tokio::spawn(async move {
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
            if let Err(error) = side.consumer.commit_consumer_state(CommitMode::Sync) {
                tracing::debug!(%error, "the Kafka link had no offset to commit at close");
            }
        }
        if let Some(side) = client {
            side.close();
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

fn producer(base: &ClientConfig) -> Result<FutureProducer, BoxError> {
    let mut config = base.clone();
    config.set("message.max.bytes", MAX_MESSAGE.to_string()).set("message.timeout.ms", DELIVERY_TIMEOUT);
    Ok(config.create()?)
}

/// A consumer of the control topic assigned every partition from its end, outside any group's
/// rebalancing, so it reads from `bind` on rather than from a first assignment seconds later. Its
/// `group.id` is this instance's own and it commits nothing.
async fn control_consumer(base: &ClientConfig, group: &str) -> Result<Arc<StreamConsumer>, BoxError> {
    let mut config = base.clone();
    config
        .set("group.id", format!("{group}.control.{}", uuid::Uuid::new_v4().simple()))
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "latest");
    let consumer: Arc<StreamConsumer> = Arc::new(config.create()?);
    let reader = Arc::clone(&consumer);
    // `fetch_metadata` blocks the calling thread.
    let partitions: Vec<i32> = tokio::task::spawn_blocking(move || -> Result<Vec<i32>, BoxError> {
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
    consumer: Arc<StreamConsumer>,
    producer: FutureProducer,
    codec: Codec,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
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
        // reads the record again.
        let ack = Ack::new(move |_accepted| {
            if let Err(error) = consumer.store_offset(&topic, partition, offset) {
                tracing::warn!(%error, topic, "the Kafka link could not store an offset");
            }
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
                if let Err(error) = produce(&self.producer, &reply, correlation.as_deref(), Some(OPENED), &CallHeaders::new(), &[]).await {
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
        let producer = self.producer.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let released = key.clone();
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let producer = producer.clone();
            let calls = Arc::clone(&calls);
            let reply = reply.clone();
            let correlation = correlation.clone();
            let released = released.clone();
            Box::pin(async move {
                if is_terminal(&frame) {
                    calls.release(&released);
                }
                let bytes = codec.encode_frame(&frame)?;
                produce(&producer, &reply, correlation.as_deref(), None, &CallHeaders::new(), &bytes).await
            })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

/// The group consumer's records until the drain, which pauses the partitions and leaves the
/// consumer to the acknowledgments still owed.
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
            Ok(record) => side.on_request(record).await,
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn control_lane(side: Arc<ServerSide>, control: Arc<StreamConsumer>) {
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

/// The client side: the producer, the reply topic every reply lands on, each call's correlation
/// id `<id>.<call>`.
struct ClientSide {
    producer: FutureProducer,
    codec: Codec,
    id: String,
    reply_topic: String,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Stops the reply router at close.
    closed: Mutex<Option<oneshot::Sender<()>>>,
}

enum ClientCall {
    Unary,
    /// A streamed request: its `in`, `in_end` and `cancel` frames wait in `queue` until the server
    /// acknowledges the `open`, then go out in order.
    Streaming { gate: Option<oneshot::Sender<()>>, queue: mpsc::UnboundedSender<Frame> },
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame) -> Result<(), BoxError> {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                lock(&self.calls).insert(id, ClientCall::Unary);
                let sent = self.produce_request(&pattern, id, &headers, None, data.as_bytes()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                let mut record_headers = OwnedHeaders::new();
                record_headers = append(record_headers, &headers);
                let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(data.as_bytes()).headers(record_headers);
                deliver(&self.producer, record, pattern.as_str(), data.len()).await
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { gate: Some(gate), queue });
                tokio::spawn(pump(Arc::clone(self), id, opened, queued));
                let sent = self.produce_request(&pattern, id, &headers, Some(OPEN), &[]).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
                if data.len() as u64 > MAX_MESSAGE {
                    return Err(Box::new(FrameTooLarge { size: data.len() as u64, limit: MAX_MESSAGE }));
                }
                self.enqueue(id, Frame::In { id, data });
                Ok(())
            }
            Frame::InEnd { id } => {
                self.enqueue(id, Frame::InEnd { id });
                Ok(())
            }
            Frame::Cancel { id } => {
                let call = lock(&self.calls).remove(&id);
                match call {
                    Some(ClientCall::Unary) => self.produce_control(id, CANCEL, &[]).await,
                    Some(ClientCall::Streaming { queue, .. }) => {
                        let _ = queue.send(Frame::Cancel { id });
                        Ok(())
                    }
                    None => Ok(()),
                }
            }
            other => Err(format!("a client does not send a `{}` frame", other.kind()).into()),
        }
    }

    fn correlation(&self, call: u64) -> String {
        format!("{}.{call}", self.id)
    }

    fn call_of(&self, correlation: &str) -> Option<u64> {
        correlation.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    /// Keyed by this client's id, so one caller's requests share a partition and stay ordered.
    async fn produce_request(&self, pattern: &Pattern, call: u64, headers: &CallHeaders, kind: Option<&str>, body: &[u8]) -> Result<(), BoxError> {
        let correlation = self.correlation(call);
        let mut record_headers = OwnedHeaders::new()
            .insert(Header { key: REPLY_TO, value: Some(self.reply_topic.as_str()) })
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) });
        if let Some(kind) = kind {
            record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
        }
        record_headers = append(record_headers, headers);
        let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(body).headers(record_headers);
        deliver(&self.producer, record, pattern.as_str(), body.len()).await
    }

    async fn produce_control(&self, call: u64, kind: &str, body: &[u8]) -> Result<(), BoxError> {
        let correlation = self.correlation(call);
        let record_headers = OwnedHeaders::new()
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) })
            .insert(Header { key: KIND, value: Some(kind) });
        let record = FutureRecord::to(CONTROL).key(correlation.as_str()).payload(body).headers(record_headers);
        deliver(&self.producer, record, CONTROL, body.len()).await
    }

    fn enqueue(&self, id: u64, frame: Frame) {
        if let Some(ClientCall::Streaming { queue, .. }) = lock(&self.calls).get(&id) {
            let _ = queue.send(frame);
        }
    }

    fn open_gate(&self, id: u64) {
        if let Some(ClientCall::Streaming { gate, .. }) = lock(&self.calls).get_mut(&id)
            && let Some(gate) = gate.take()
        {
            let _ = gate.send(());
        }
    }

    fn close(&self) {
        if let Some(closed) = lock(&self.closed).take() {
            let _ = closed.send(());
        }
    }
}

async fn route_replies(
    side: Arc<ClientSide>,
    replies: StreamConsumer,
    frames: mpsc::UnboundedSender<Frame>,
    mut closing: oneshot::Receiver<()>,
) {
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
            side.open_gate(call);
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
}

/// Publishes one streamed request's control frames once the server has acknowledged its `open`.
async fn pump(side: Arc<ClientSide>, call: u64, opened: oneshot::Receiver<()>, mut queued: mpsc::UnboundedReceiver<Frame>) {
    if opened.await.is_err() {
        return;
    }
    while let Some(frame) = queued.recv().await {
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => continue,
        };
        if let Err(error) = side.produce_control(call, kind, &body).await {
            tracing::debug!(%error, kind, "the Kafka link could not publish a control frame");
        }
    }
}

/// One reply-lane record, keyed by the call's correlation id so one call's frames share a
/// partition and stay ordered.
async fn produce(
    producer: &FutureProducer,
    topic: &str,
    correlation: Option<&str>,
    kind: Option<&str>,
    headers: &CallHeaders,
    body: &[u8],
) -> Result<(), BoxError> {
    let mut record_headers = OwnedHeaders::new();
    if let Some(correlation) = correlation {
        record_headers = record_headers.insert(Header { key: CORRELATION, value: Some(correlation) });
    }
    if let Some(kind) = kind {
        record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
    }
    record_headers = append(record_headers, headers);
    let record = FutureRecord::to(topic).key(correlation.unwrap_or_default()).payload(body).headers(record_headers);
    deliver(producer, record, topic, body.len()).await
}

/// Produces `record` and waits for its delivery report. An unknown topic is the miss signal
/// where the broker does not auto-create topics; an oversized record is `FrameTooLarge`.
async fn deliver(producer: &FutureProducer, record: FutureRecord<'_, str, [u8]>, topic: &str, size: usize) -> Result<(), BoxError> {
    if size as u64 > MAX_MESSAGE {
        return Err(Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }));
    }
    match producer.send(record, Timeout::After(Duration::from_secs(5))).await {
        Ok(_) => Ok(()),
        Err((KafkaError::MessageProduction(RDKafkaErrorCode::UnknownTopicOrPartition | RDKafkaErrorCode::UnknownTopic), _)) => {
            Err(Box::new(NoDestination { pattern: topic.to_owned() }))
        }
        Err((KafkaError::MessageProduction(RDKafkaErrorCode::MessageSizeTooLarge), _)) => {
            Err(Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }))
        }
        Err((error, _)) => Err(Box::new(error)),
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
