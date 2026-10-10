use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use rumqttc::v5::mqttbytes::QoS as MqttQoS;
use rumqttc::v5::mqttbytes::v5::{
    Filter, Packet, PubAckReason, PubRecReason, Publish, PublishProperties, SubscribeReasonCode, UnsubAck, UnsubAckReason,
};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use rumqttc::{Outgoing, Transport};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use ulo::{AppHandle, BoxError, BoxFuture};
use ulo_tokio::Tokio;
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::{Detail, Details, ErrorKind};

/// The user property naming a message's frame kind where the message alone does not tell it:
/// `open` on the request lane, `in`, `in_end` and `cancel` on the control lane, `opened` on the
/// reply lane. A request-lane message without it is a `req` when it carries a Response Topic and
/// an `evt` when it does not.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The topic every server instance subscribes to outside any shared subscription, so each sees
/// every control message and acts on the ones whose Correlation Data names a call it holds.
const CONTROL: &str = "ulo/rpc/control";

/// The `ErrorInfo` domain of the `no_destination` error this link writes for PUBACK 0x10.
const DOMAIN: &str = "ulo.rpc";

/// The largest packet this link accepts from the broker, MQTT's own ceiling; rumqttc's default
/// of 10 KiB would refuse ordinary replies.
const MAX_INCOMING: u32 = 268_435_455;

/// The Receive Maximum this link announces in its CONNECT, MQTT 5's largest: the QoS 1 and 2
/// publishes the broker may have unacknowledged to it at once. rumqttc acknowledges a publish as
/// its event loop reads it, so the window bounds only what is on its way; the server's own
/// in-flight bound, `Server::max_inflight`, refuses what it will not take. MQTT 5 reads an absent
/// Receive Maximum as 65,535, but Mosquitto 2.0 then applies its own `max_inflight_messages`, 20
/// unset, and holds the rest back, and what it holds back is dropped with the session when the
/// instance leaves. The crate doc says why the link refuses rather than holds work at the broker.
const RECEIVE_MAXIMUM: u16 = u16::MAX;

/// The MQTT v5 link.
///
/// The link's connections and tasks live on the tokio runtime it holds, the one current where it
/// was built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may
/// be polled on any executor or on a plain thread: each connection's event loop runs there, and a
/// publish, an unsubscribe or a disconnect reaches it through rumqttc's request channel. A link
/// built outside a runtime and given none refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
pub struct Mqtt {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) qos: QoS,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The CONNACK's Maximum Packet Size, zero until a connection reads one.
    pub(crate) max_packet: Arc<AtomicU64>,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, JoinHandle<()>)>,
    client: Option<(Arc<ClientSide>, JoinHandle<()>)>,
}

impl Mqtt {
    /// The link on `url`, `mqtt://host:1883` or `mqtts://host:8883`, parsed in `prepare` and
    /// connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        Mqtt {
            url: url.into(),
            group: None,
            qos: QoS::AtLeastOnce,
            codec: Codec::Json,
            runtime: Tokio::try_current(),
            default_group: None,
            max_packet: Arc::new(AtomicU64::new(0)),
            state: Mutex::new(State::default()),
        }
    }

    /// The shared-subscription group server instances share, in place of the root module's full
    /// type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The QoS every publish and subscription uses: `AtLeastOnce` unset. At `AtMostOnce` a miss
    /// has no signal and is the client's `Timeout`.
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The tokio runtime the link's connections and tasks run on, in place of the one current
    /// where it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the MQTT link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

/// An MQTT quality of service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QoS {
    /// QoS 0: nothing is acknowledged, so a miss has no signal.
    AtMostOnce,
    /// QoS 1, with PUBACK.
    #[default]
    AtLeastOnce,
    /// QoS 2, with PUBREC.
    ExactlyOnce,
}

impl QoS {
    fn wire(self) -> MqttQoS {
        match self {
            QoS::AtMostOnce => MqttQoS::AtMostOnce,
            QoS::AtLeastOnce => MqttQoS::AtLeastOnce,
            QoS::ExactlyOnce => MqttQoS::ExactlyOnce,
        }
    }
}

impl Link for Mqtt {
    const NAME: &'static str = "mqtt";

    fn capabilities(&self) -> Capabilities {
        let max_packet = self.max_packet.load(Ordering::Relaxed);
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerTopic)
            .miss_signal(self.qos != QoS::AtMostOnce)
            .max_frame((max_packet > 0).then_some(max_packet))
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.usable()?;
        Target::parse(&self.url)?;
        match &self.group {
            Some(group) => check_group(group)?,
            None => self.default_group = Some(share_name(&format!("{:#}", app.root().name()))),
        }
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the MQTT link's group is unset: `listen` ran before `prepare`")?;
        let runtime = self.runtime()?;
        let target = Target::parse(&self.url)?;
        let (client, eventloop) = AsyncClient::new(target.options(client_id()), 64);
        let shared: Vec<String> = patterns.iter().map(|pattern| format!("$share/{group}/{pattern}")).collect();
        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let side = Arc::new(ServerSide {
            client,
            codec: self.codec,
            qos: self.qos.wire(),
            runtime: runtime.clone(),
            shared,
            calls: Arc::new(Calls::new()),
            phase,
            max_packet: Arc::clone(&self.max_packet),
            deliveries: Mutex::new(Some(deliveries)),
            unsubscribing: Mutex::new(Unsubscribing::default()),
            unconfirmed: watch::channel(0).0,
        });

        let (ready, bound) = oneshot::channel();
        let task = runtime.handle().spawn(server_loop(Arc::clone(&side), eventloop, ready));
        // Until the broker answers, a `listen` dropped mid-handshake takes the event loop with it.
        let pending = AbortOnDrop::new(task.abort_handle());
        // `bind`'s outcome waits for the first CONNACK and the SUBACK of every subscription, so a
        // broker without shared subscriptions, or one refusing a filter, fails `bind`.
        let outcome = bound.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        if let Err(error) = outcome {
            let _ = side.client.try_disconnect();
            return Err(error);
        }
        pending.disarm();
        lock(&self.state).server = Some((side, task));
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let target = Target::parse(&self.url)?;
        let id = client_id();
        let reply_topic = format!("ulo/rpc/reply/{id}");
        let (client, eventloop) = AsyncClient::new(target.options(id.clone()), 64);
        let side = Arc::new(ClientSide {
            client,
            codec: self.codec,
            qos: self.qos.wire(),
            runtime: runtime.clone(),
            id,
            reply_topic,
            max_packet: Arc::clone(&self.max_packet),
            calls: Mutex::new(HashMap::new()),
            order: tokio::sync::Mutex::new(()),
            outgoing: Mutex::new(None),
            pkids: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let (frames, replies) = mpsc::unbounded_channel();
        let (ready, subscribed) = oneshot::channel();
        let task = runtime.handle().spawn(client_loop(Arc::clone(&side), eventloop, frames, ready));
        // Until the broker answers, a `connect` dropped mid-handshake, a call's timeout cutting it
        // short, takes the event loop with it.
        let pending = AbortOnDrop::new(task.abort_handle());
        // The reply subscription is in place before the first request can be published.
        let outcome = subscribed.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        outcome?;
        pending.disarm();
        lock(&self.state).client = Some((Arc::clone(&side), task));

        Ok(Outbound {
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let side = Arc::clone(&side);
                Box::pin(async move { side.send(pattern, frame).await })
            }),
            replies: receiver_stream(replies),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.as_ref().map(|(side, _)| Arc::clone(side));
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        for filter in &side.shared {
            side.unsubscribe_queued(filter);
            if let Err(error) = side.client.unsubscribe(filter.clone()).await {
                side.unsubscribe_refused();
                tracing::warn!(%error, filter, "the MQTT link could not unsubscribe");
            }
        }
        // The broker routes requests here until it has processed each UNSUBSCRIBE, and writes
        // every PUBLISH it routed before that ahead of the UNSUBACK, which the event loop reads in
        // order: once every UNSUBACK is in, nothing more is routed here and what was routed has
        // been delivered. MQTT 5 §3.10.4 lets a broker deliver after the UNSUBACK a publish it held
        // back for this instance's flow control, which only more than `RECEIVE_MAXIMUM`
        // unacknowledged publishes, or a smaller cap the broker applies, would leave it holding.
        // No bound of its own: the core drops this future at the drain's deadline.
        let unconfirmed = Unconfirmed(Some(Arc::clone(&side)));
        let _ = side.unconfirmed.subscribe().wait_for(|filters| *filters == 0).await;
        unconfirmed.disarm();
        let watched = Arc::clone(&side);
        side.runtime.handle().spawn(async move {
            let mut count = watched.calls.count.subscribe();
            while count.wait_for(|held| *held == 0).await.is_ok() {
                if watched.end_if_idle() {
                    return;
                }
            }
        });
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (server, client) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take())
        };
        if let Some((side, task)) = server {
            side.phase.send_replace(Phase::Closed);
            // Taken before the table is cleared, so no call is held once it is.
            lock(&side.deliveries).take();
            side.calls.clear();
            shut(&side.client, task).await;
        }
        if let Some((side, task)) = client {
            side.closed.store(true, Ordering::Release);
            shut(&side.client, task).await;
        }
        Ok(())
    }
}

/// Queues a DISCONNECT and awaits the connection's event loop, which ends once it reports the
/// DISCONNECT written, or once the connection fails after `close`. `publish` and `disconnect` only
/// queue requests for the loop, which writes them in order, so every one queued earlier is written
/// ahead of the DISCONNECT: a drained call's reply or a refusal on the server side, an event or a
/// `cancel` on the client side. Where neither ending comes, the core's `close` bound drops this
/// future and the guard aborts the loop.
async fn shut(client: &AsyncClient, mut task: JoinHandle<()>) {
    let _abort = AbortOnDrop::new(task.abort_handle());
    let _ = client.disconnect().await;
    let _ = (&mut task).await;
}

/// Logs, when the drain's future is dropped before the broker confirmed every UNSUBSCRIBE, the
/// filters it had not confirmed. The core drops that future at the drain's deadline; the broker
/// can then still route requests here, and those reaching the event loop after `close` queued its
/// DISCONNECT go unanswered.
struct Unconfirmed(Option<Arc<ServerSide>>);

impl Unconfirmed {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for Unconfirmed {
    fn drop(&mut self) {
        let Some(side) = self.0.take() else { return };
        let filters = side.unconfirmed_filters();
        if !filters.is_empty() {
            tracing::warn!(
                ?filters,
                "the drain's deadline passed before the MQTT broker confirmed the unsubscribe; until it does it can route requests to this instance"
            );
        }
    }
}

/// Aborts a task when dropped, unless disarmed first.
struct AbortOnDrop(Option<AbortHandle>);

impl AbortOnDrop {
    fn new(task: AbortHandle) -> Self {
        AbortOnDrop(Some(task))
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// Where the link connects, parsed from its url.
struct Target {
    host: String,
    port: u16,
    tls: bool,
    credentials: Option<(String, String)>,
}

impl Target {
    fn parse(url: &str) -> Result<Target, BoxError> {
        let invalid = |why: &str| -> BoxError { format!("the MQTT link's url does not parse: {why}").into() };
        let (scheme, rest) = url.split_once("://").ok_or_else(|| invalid("no scheme, expected `mqtt://` or `mqtts://`"))?;
        let (tls, default_port) = match scheme {
            "mqtt" | "tcp" => (false, 1883),
            "mqtts" | "ssl" => (true, 8883),
            other => return Err(invalid(&format!("the scheme `{other}` is not `mqtt` or `mqtts`"))),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (credentials, address) = match authority.rsplit_once('@') {
            Some((credentials, address)) => {
                let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
                (Some((user.to_owned(), password.to_owned())), address)
            }
            None => (None, authority),
        };
        let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
            let (host, after) = bracketed.split_once(']').ok_or_else(|| invalid("an unclosed `[` in the host"))?;
            let port = match after.strip_prefix(':') {
                Some(port) => port.parse().map_err(|_| invalid("the port is not a number"))?,
                None if after.is_empty() => default_port,
                None => return Err(invalid("text after the host")),
            };
            (host.to_owned(), port)
        } else {
            match address.rsplit_once(':') {
                Some((host, port)) => (host.to_owned(), port.parse().map_err(|_| invalid("the port is not a number"))?),
                None => (address.to_owned(), default_port),
            }
        };
        if host.is_empty() {
            return Err(invalid("no host"));
        }
        Ok(Target { host, port, tls, credentials })
    }

    fn options(&self, client_id: String) -> MqttOptions {
        let mut options = MqttOptions::new(client_id, self.host.clone(), self.port);
        options.set_keep_alive(Duration::from_secs(10));
        options.set_max_packet_size(Some(MAX_INCOMING));
        options.set_receive_maximum(Some(RECEIVE_MAXIMUM));
        if let Some((user, password)) = &self.credentials {
            options.set_credentials(user.clone(), password.clone());
        }
        if self.tls {
            options.set_transport(Transport::tls_with_default_config());
        }
        options
    }
}

/// The server side of a bound link. The event loop owns the connection; replies are queued on
/// `client` from whichever task answers.
struct ServerSide {
    client: AsyncClient,
    codec: Codec,
    qos: MqttQoS,
    /// The link's runtime, which the drain's watcher is spawned on.
    runtime: Tokio,
    /// The `$share/<group>/<pattern>` filters, unsubscribed by the drain.
    shared: Vec<String>,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    max_packet: Arc<AtomicU64>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream. A call is
    /// held or released, and its frame delivered, under this lock, so the drain cannot end the
    /// stream between the two: a `cancel` releasing the last held call is delivered first.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    /// The drain's UNSUBSCRIBEs the broker has not acknowledged.
    unsubscribing: Mutex<Unsubscribing>,
    /// How many filters `unsubscribing` holds, which the drain waits to reach zero.
    unconfirmed: watch::Sender<usize>,
}

/// The drain's UNSUBSCRIBEs, from the request rumqttc queues to the broker's UNSUBACK. The server
/// side unsubscribes nowhere else, so every `Outgoing::Unsubscribe` its event loop reports is the
/// drain's, and rumqttc takes requests in the order they were queued.
#[derive(Default)]
struct Unsubscribing {
    /// Filters queued, oldest first, before the event loop has given them a packet id.
    queued: VecDeque<String>,
    /// Filters by the packet id of the UNSUBSCRIBE carrying them, awaiting its UNSUBACK.
    sent: HashMap<u16, String>,
}

impl Unsubscribing {
    fn len(&self) -> usize {
        self.queued.len() + self.sent.len()
    }
}

impl ServerSide {
    fn unsubscribing(&self, change: impl FnOnce(&mut Unsubscribing)) {
        let mut unsubscribing = lock(&self.unsubscribing);
        change(&mut unsubscribing);
        self.unconfirmed.send_replace(unsubscribing.len());
    }

    /// Before the drain queues the UNSUBSCRIBE for `filter`, so the event loop's report of it
    /// finds the filter.
    fn unsubscribe_queued(&self, filter: &str) {
        self.unsubscribing(|unsubscribing| unsubscribing.queued.push_back(filter.to_owned()));
    }

    /// The request for the filter queued last never reached the event loop.
    fn unsubscribe_refused(&self) {
        self.unsubscribing(|unsubscribing| {
            unsubscribing.queued.pop_back();
        });
    }

    /// The event loop gave the oldest queued UNSUBSCRIBE packet id `pkid`.
    fn unsubscribe_sent(&self, pkid: u16) {
        self.unsubscribing(|unsubscribing| {
            if let Some(filter) = unsubscribing.queued.pop_front() {
                unsubscribing.sent.insert(pkid, filter);
            }
        });
    }

    /// The broker acknowledged the UNSUBSCRIBE with `unsuback.pkid`. A refusal leaves the broker
    /// routing to this instance, which nothing can change before close; it is logged.
    fn unsubscribe_acknowledged(&self, unsuback: &UnsubAck) {
        self.unsubscribing(|unsubscribing| {
            let Some(filter) = unsubscribing.sent.remove(&unsuback.pkid) else { return };
            if let Some(reason) = unsuback.reasons.iter().find(|reason| !matches!(reason, UnsubAckReason::Success | UnsubAckReason::NoSubscriptionExisted)) {
                tracing::warn!(filter, ?reason, "the MQTT broker refused to unsubscribe the draining server; it can still route requests to this instance");
            }
        });
    }

    /// A CONNACK without a session present: the broker holds no subscription of this client's,
    /// so nothing the drain waits for is routed here any more.
    fn session_ended(&self) {
        self.unsubscribing(|unsubscribing| {
            unsubscribing.queued.clear();
            unsubscribing.sent.clear();
        });
    }

    fn unconfirmed_filters(&self) -> Vec<String> {
        let unsubscribing = lock(&self.unsubscribing);
        let mut filters: Vec<String> = unsubscribing.sent.values().chain(unsubscribing.queued.iter()).cloned().collect();
        filters.sort();
        filters
    }

    /// Ends the inbound stream if no call is held, answering whether it has ended.
    fn end_if_idle(&self) -> bool {
        let mut deliveries = lock(&self.deliveries);
        if !self.calls.is_empty() {
            return false;
        }
        deliveries.take();
        true
    }

    /// Answers a request or streamed request that arrived once the inbound stream ended `err` of
    /// kind `unavailable`, as the server answers one during the drain. The broker routes a request
    /// to this instance until it has processed the drain's UNSUBSCRIBE, which rumqttc queues
    /// behind the replies already waiting, and dropping it would leave the caller to its own
    /// `Timeout`. Queued without waiting: this runs on the event loop, which drains the queue.
    fn refuse(&self, reply: String, correlation: Option<Bytes>, pattern: &str) {
        let error = ErrorBody::new(ErrorKind::Unavailable, "the server is shutting down", Details::new());
        let sent = self.codec.encode_frame(&Frame::Err { id: 0, error }).map_err(BoxError::from).and_then(|bytes| {
            let properties = reply_properties(correlation, None);
            Ok(self.client.try_publish_with_properties(reply, self.qos, false, bytes, properties)?)
        });
        if let Err(error) = sent {
            tracing::warn!(%error, pattern, "the MQTT link could not refuse a request that arrived after the drain");
        }
    }

    fn on_publish(&self, publish: Publish) {
        let topic = String::from_utf8_lossy(&publish.topic).into_owned();
        let properties = publish.properties.unwrap_or_default();
        let kind = kind_of(&properties.user_properties);
        let key = properties
            .correlation_data
            .as_ref()
            .map(|data| String::from_utf8_lossy(data).into_owned())
            .or_else(|| properties.response_topic.clone());
        let deliveries = lock(&self.deliveries);
        if topic == CONTROL {
            let Some(key) = key else { return };
            let Some(sender) = deliveries.as_ref() else { return };
            let frame = match kind.as_deref() {
                Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(publish.payload) }, path)),
                Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
                Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
                _ => None,
            };
            if let Some((frame, path)) = frame {
                let _ = sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
            }
            return;
        }

        let headers = call_headers(&properties.user_properties);
        let reply = properties.response_topic.clone().map(|topic| (topic, properties.correlation_data.clone()));
        match (reply, kind.as_deref()) {
            (None, None) => {
                let frame = Frame::Evt { pattern: topic, headers, data: Data::new(publish.payload) };
                if let Some(sender) = deliveries.as_ref() {
                    let _ = sender.send(Delivery { frame, reply: None, ack: Ack::none() });
                }
            }
            (Some((reply, correlation)), None) => {
                let key = key.unwrap_or_else(|| reply.clone());
                if let Some(sender) = deliveries.as_ref() {
                    let (id, path) = self.hold(key.clone(), reply.clone(), correlation.clone());
                    let frame = Frame::Req { id, pattern: topic.clone(), headers, data: Data::new(publish.payload) };
                    if sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() }).is_ok() {
                        return;
                    }
                    self.calls.release(&key);
                }
                drop(deliveries);
                self.refuse(reply, correlation, &topic);
            }
            (Some((reply, correlation)), Some(OPEN)) => {
                // Checked before `opened` is sent, so the caller is not told `opened` for a call
                // the server never receives.
                let Some(sender) = deliveries.as_ref() else {
                    drop(deliveries);
                    return self.refuse(reply, correlation, &topic);
                };
                let key = key.unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply.clone(), correlation.clone());
                // The caller holds the request's items until this arrives. Queued without waiting:
                // this runs on the event loop, which is what drains the queue.
                let opened = reply_properties(correlation, Some(OPENED));
                if let Err(error) = self.client.try_publish_with_properties(reply, self.qos, false, Bytes::new(), opened) {
                    tracing::warn!(%error, pattern = topic, "the MQTT link could not acknowledge a streamed request");
                }
                let _ = sender.send(Delivery { frame: Frame::Open { id, pattern: topic, headers }, reply: Some(path), ack: Ack::none() });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern = topic, kind, "the MQTT link dropped a request-lane message of an unknown kind");
            }
        }
    }

    /// Holds a call under `key`, its Correlation Data, the key its control messages carry.
    fn hold(&self, key: String, reply: String, correlation: Option<Bytes>) -> (u64, ReplyPath) {
        let client = self.client.clone();
        let codec = self.codec;
        let qos = self.qos;
        let calls = Arc::clone(&self.calls);
        let released = key.clone();
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let client = client.clone();
            let calls = Arc::clone(&calls);
            let reply = reply.clone();
            let released = released.clone();
            let properties = reply_properties(correlation.clone(), None);
            Box::pin(async move {
                if is_terminal(&frame) {
                    calls.release(&released);
                }
                let bytes = codec.encode_frame(&frame)?;
                client.publish_with_properties(reply, qos, false, bytes, properties).await?;
                Ok(())
            })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

/// Polls the server connection until it has written the DISCONNECT `close` queued, or until the
/// connection fails after `close`. On every CONNACK it subscribes again, rumqttc keeping no
/// subscription across a reconnect: the shared filters while serving, the control topic always.
async fn server_loop(side: Arc<ServerSide>, mut eventloop: EventLoop, ready: oneshot::Sender<Result<(), BoxError>>) {
    let mut ready = Some(ready);
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(connack))) => {
                if !connack.session_present {
                    side.session_ended();
                }
                let properties = connack.properties.as_ref();
                if let Some(max) = properties.and_then(|properties| properties.max_packet_size) {
                    side.max_packet.store(u64::from(max), Ordering::Relaxed);
                }
                if properties.and_then(|properties| properties.shared_subscription_available) == Some(0) {
                    let error = "the MQTT broker's CONNACK announces no shared-subscription support, so the link's \
                                 `Competing` delivery through `$share/<group>/` (the `Mqtt::group` setting) would be false";
                    match ready.take() {
                        Some(ready) => {
                            let _ = ready.send(Err(error.into()));
                            return;
                        }
                        None => tracing::error!("{error}; after a reconnect the link stays subscribed to the control topic alone"),
                    }
                    let _ = side.client.try_subscribe(CONTROL, side.qos);
                    continue;
                }
                let serving = *side.phase.borrow() == Phase::Serving;
                let mut filters: Vec<Filter> = Vec::new();
                if serving {
                    filters.extend(side.shared.iter().map(|filter| Filter::new(filter.clone(), side.qos)));
                }
                filters.push(Filter::new(CONTROL, side.qos));
                if let Err(error) = side.client.try_subscribe_many(filters)
                    && let Some(ready) = ready.take()
                {
                    let _ = ready.send(Err(format!("the MQTT link could not subscribe: {error}").into()));
                    return;
                }
            }
            Ok(Event::Incoming(Packet::SubAck(suback))) => {
                let Some(pending) = ready.take() else { continue };
                let refused = suback.return_codes.iter().enumerate().find_map(|(index, code)| match code {
                    SubscribeReasonCode::Success(_) => None,
                    code => Some((side.shared.get(index).map_or(CONTROL, String::as_str), code.clone())),
                });
                match refused {
                    None => {
                        let _ = pending.send(Ok(()));
                    }
                    Some((filter, code)) => {
                        let _ = pending.send(Err(format!("the MQTT broker refused the subscription `{filter}`: {code:?}").into()));
                        return;
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => side.on_publish(publish),
            Ok(Event::Outgoing(Outgoing::Unsubscribe(pkid))) => side.unsubscribe_sent(pkid),
            Ok(Event::Incoming(Packet::UnsubAck(unsuback))) => side.unsubscribe_acknowledged(&unsuback),
            // rumqttc also writes a DISCONNECT of its own on a protocol error, after which the
            // broker closes the connection and the loop reconnects.
            Ok(Event::Outgoing(Outgoing::Disconnect)) if *side.phase.borrow() == Phase::Closed => return,
            Ok(_) => {}
            Err(error) => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
                    return;
                }
                if *side.phase.borrow() == Phase::Closed {
                    tracing::debug!(%error, "the MQTT link's connection failed after `close`");
                    return;
                }
                tracing::warn!(%error, "the MQTT link lost its connection; reconnecting");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// The calls a server holds, by the key their control messages carry, each with its local id and
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

    fn is_empty(&self) -> bool {
        lock(&self.held).is_empty()
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

/// The client side: one reply topic for every reply, each call's Correlation Data `<id>.<call>`.
struct ClientSide {
    client: AsyncClient,
    codec: Codec,
    qos: MqttQoS,
    /// The link's runtime, which a streamed request's pump is spawned on.
    runtime: Tokio,
    id: String,
    reply_topic: String,
    max_packet: Arc<AtomicU64>,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Held across one publish and its `Outgoing::Publish` event, so the event names that
    /// publish's packet id: rumqttc's `publish` returns before the id is assigned.
    order: tokio::sync::Mutex<()>,
    /// The publish waiting for its packet id, which is sent `None` when the connection fails
    /// first.
    outgoing: Mutex<Option<Waiting>>,
    /// The packet id of each request publish awaiting its PUBACK or PUBREC, and its call. The
    /// event loop writes the entry on the publish's `Outgoing::Publish` event: rumqttc has written
    /// the packet by then, so the PUBACK can be read before the publishing task runs again, and
    /// an entry written there would miss it and lose the miss signal.
    pkids: Mutex<HashMap<u16, u64>>,
    /// Set by `close` before it queues the DISCONNECT, so the event loop ends on writing it.
    closed: AtomicBool,
}

/// A publish waiting for its `Outgoing::Publish` event.
struct Waiting {
    /// The request's call, `None` for an event or a control message.
    call: Option<u64>,
    assigned: oneshot::Sender<Option<u16>>,
}

enum ClientCall {
    Unary { pattern: Pattern },
    /// A streamed request: its `in`, `in_end` and `cancel` frames wait in `queue` until the server
    /// acknowledges the `open`, then go out in order.
    Streaming { pattern: Pattern, gate: Option<oneshot::Sender<()>>, queue: mpsc::UnboundedSender<Frame> },
    /// A streamed request cancelled before the server acknowledged its `open`: its `cancel` waits
    /// in the pump's queue and goes out alone once the acknowledgment opens `gate`, the frames
    /// queued before it dropped. The entry stays until then, or until the link closes when no
    /// acknowledgment comes.
    Cancelled { pattern: Pattern, gate: oneshot::Sender<()> },
}

impl ClientCall {
    fn pattern(&self) -> &Pattern {
        match self {
            ClientCall::Unary { pattern } | ClientCall::Streaming { pattern, .. } | ClientCall::Cancelled { pattern, .. } => pattern,
        }
    }
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame) -> Result<(), BoxError> {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                self.fits(pattern.as_str(), data.len())?;
                lock(&self.calls).insert(id, ClientCall::Unary { pattern: pattern.clone() });
                let properties = self.request_properties(id, &headers, None);
                let sent = self.publish(pattern.to_string(), properties, data.into_bytes(), Some(id)).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                self.fits(pattern.as_str(), data.len())?;
                let properties = PublishProperties { user_properties: user_properties(&headers, None), ..Default::default() };
                self.publish(pattern.to_string(), properties, data.into_bytes(), None).await
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { pattern: pattern.clone(), gate: Some(gate), queue });
                self.runtime.handle().spawn(pump(Arc::clone(self), id, opened, queued));
                let properties = self.request_properties(id, &headers, Some(OPEN));
                let sent = self.publish(pattern.to_string(), properties, Bytes::new(), Some(id)).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
                self.fits(CONTROL, data.len())?;
                self.enqueue(id, Frame::In { id, data });
                Ok(())
            }
            Frame::InEnd { id } => {
                self.enqueue(id, Frame::InEnd { id });
                Ok(())
            }
            Frame::Cancel { id } => {
                let unary = {
                    let mut calls = lock(&self.calls);
                    match calls.remove(&id) {
                        Some(ClientCall::Unary { .. }) => true,
                        // The server takes the `open` and holds the call, so the `cancel` follows
                        // once it acknowledges it.
                        Some(ClientCall::Streaming { pattern, gate: Some(gate), queue }) => {
                            let _ = queue.send(Frame::Cancel { id });
                            calls.insert(id, ClientCall::Cancelled { pattern, gate });
                            false
                        }
                        Some(ClientCall::Streaming { gate: None, queue, .. }) => {
                            let _ = queue.send(Frame::Cancel { id });
                            false
                        }
                        Some(cancelled @ ClientCall::Cancelled { .. }) => {
                            calls.insert(id, cancelled);
                            false
                        }
                        None => false,
                    }
                };
                if unary { self.publish_control(id, CANCEL, Bytes::new()).await } else { Ok(()) }
            }
            other => Err(format!("a client does not send a `{}` frame", other.kind()).into()),
        }
    }

    fn correlation(&self, call: u64) -> Bytes {
        Bytes::from(format!("{}.{call}", self.id))
    }

    fn call_of(&self, correlation: &[u8]) -> Option<u64> {
        let text = std::str::from_utf8(correlation).ok()?;
        text.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    fn request_properties(&self, call: u64, headers: &CallHeaders, kind: Option<&str>) -> PublishProperties {
        PublishProperties {
            response_topic: Some(self.reply_topic.clone()),
            correlation_data: Some(self.correlation(call)),
            user_properties: user_properties(headers, kind),
            ..Default::default()
        }
    }

    /// A packet's size beside the broker's Maximum Packet Size: the body, the topic and a margin
    /// for the fixed header and properties.
    fn fits(&self, topic: &str, body: usize) -> Result<(), BoxError> {
        let limit = self.max_packet.load(Ordering::Relaxed);
        let size = (body + topic.len() + self.reply_topic.len() + self.id.len() + 64) as u64;
        if limit > 0 && size > limit {
            return Err(Box::new(FrameTooLarge { size: body as u64, limit }));
        }
        Ok(())
    }

    async fn publish(&self, topic: String, properties: PublishProperties, body: Bytes, call: Option<u64>) -> Result<(), BoxError> {
        let _order = self.order.lock().await;
        let (assigned, pkid) = oneshot::channel();
        *lock(&self.outgoing) = Some(Waiting { call, assigned });
        if let Err(error) = self.client.publish_with_properties(topic, self.qos, false, body, properties).await {
            lock(&self.outgoing).take();
            return Err(format!("the MQTT link could not queue a publish: {error}").into());
        }
        match pkid.await {
            Ok(Some(_)) => Ok(()),
            _ => Err("the MQTT link's connection ended before the publish went out".into()),
        }
    }

    async fn publish_control(&self, call: u64, kind: &str, body: Bytes) -> Result<(), BoxError> {
        let properties = PublishProperties {
            correlation_data: Some(self.correlation(call)),
            user_properties: vec![(KIND.to_owned(), kind.to_owned())],
            ..Default::default()
        };
        self.publish(CONTROL.to_owned(), properties, body, None).await
    }

    fn enqueue(&self, id: u64, frame: Frame) {
        if let Some(ClientCall::Streaming { queue, .. }) = lock(&self.calls).get(&id) {
            let _ = queue.send(frame);
        }
    }

    fn open_gate(&self, id: u64) {
        let mut calls = lock(&self.calls);
        match calls.get_mut(&id) {
            Some(ClientCall::Streaming { gate, .. }) => {
                if let Some(gate) = gate.take() {
                    let _ = gate.send(());
                }
            }
            Some(ClientCall::Cancelled { .. }) => {
                if let Some(ClientCall::Cancelled { gate, .. }) = calls.remove(&id) {
                    let _ = gate.send(());
                }
            }
            _ => {}
        }
    }

    fn take(&self, id: u64) -> Option<ClientCall> {
        lock(&self.calls).remove(&id)
    }

    /// Fails the publish waiting for its packet id and forgets the packets awaiting an
    /// acknowledgment, once the connection has ended: no `Outgoing::Publish` and no acknowledgment
    /// will come for them.
    fn ended(&self) {
        if let Some(waiting) = lock(&self.outgoing).take() {
            let _ = waiting.assigned.send(None);
        }
        lock(&self.pkids).clear();
    }

    /// A PUBACK or PUBREC for one of this client's publishes: 0x10 on a request is the miss
    /// signal.
    fn acknowledged(&self, pkid: u16, no_subscribers: bool, frames: &mpsc::UnboundedSender<Frame>) {
        let Some(call) = lock(&self.pkids).remove(&pkid) else { return };
        if no_subscribers && let Some(entry) = self.take(call) {
            let _ = frames.send(no_destination(call, entry.pattern()));
        }
    }
}

/// Polls the client connection until it has written the DISCONNECT `close` queued, or until the
/// connection fails.
async fn client_loop(
    side: Arc<ClientSide>,
    mut eventloop: EventLoop,
    frames: mpsc::UnboundedSender<Frame>,
    ready: oneshot::Sender<Result<(), BoxError>>,
) {
    let mut ready = Some(ready);
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(connack))) => {
                if let Some(max) = connack.properties.and_then(|properties| properties.max_packet_size) {
                    side.max_packet.store(u64::from(max), Ordering::Relaxed);
                }
                if let Err(error) = side.client.try_subscribe(side.reply_topic.clone(), side.qos)
                    && let Some(ready) = ready.take()
                {
                    let _ = ready.send(Err(format!("the MQTT link could not subscribe to its reply topic: {error}").into()));
                    return;
                }
            }
            Ok(Event::Incoming(Packet::SubAck(suback))) => {
                if let Some(ready) = ready.take() {
                    let outcome = match suback.return_codes.first() {
                        Some(SubscribeReasonCode::Success(_)) => Ok(()),
                        code => Err(format!("the MQTT broker refused the reply topic subscription: {code:?}").into()),
                    };
                    let failed = outcome.is_err();
                    let _ = ready.send(outcome);
                    if failed {
                        return;
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let properties = publish.properties.unwrap_or_default();
                let Some(call) = properties.correlation_data.as_deref().and_then(|data| side.call_of(data)) else { continue };
                if kind_of(&properties.user_properties).as_deref() == Some(OPENED) {
                    side.open_gate(call);
                    continue;
                }
                let frame = match side.codec.decode_frame(&publish.payload) {
                    Ok(frame) => with_id(frame, call),
                    Err(error) => {
                        tracing::warn!(%error, "the MQTT link dropped a reply that does not decode");
                        continue;
                    }
                };
                if is_terminal(&frame) {
                    side.take(call);
                }
                let _ = frames.send(frame);
            }
            Ok(Event::Incoming(Packet::PubAck(puback))) => {
                side.acknowledged(puback.pkid, puback.reason == PubAckReason::NoMatchingSubscribers, &frames);
            }
            Ok(Event::Incoming(Packet::PubRec(pubrec))) => {
                side.acknowledged(pubrec.pkid, pubrec.reason == PubRecReason::NoMatchingSubscribers, &frames);
            }
            Ok(Event::Outgoing(Outgoing::Publish(pkid))) => {
                if let Some(Waiting { call, assigned }) = lock(&side.outgoing).take() {
                    if let Some(call) = call
                        && pkid != 0
                    {
                        lock(&side.pkids).insert(pkid, call);
                    }
                    let _ = assigned.send(Some(pkid));
                }
            }
            // A publish still waiting was queued after the DISCONNECT and is never written.
            Ok(Event::Outgoing(Outgoing::Disconnect)) if side.closed.load(Ordering::Acquire) => {
                side.ended();
                return;
            }
            Ok(_) => {}
            Err(error) => {
                // A clean session drops whatever was queued, so the waiting publish is lost and no
                // acknowledgment will come for the packets in flight.
                side.ended();
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
                    return;
                }
                if side.closed.load(Ordering::Acquire) {
                    tracing::debug!(%error, "the MQTT link's connection failed after `close`");
                    return;
                }
                // What the broker routed to the reply topic while the connection was down is gone,
                // so the reply lane ends here: `RpcClient` fails the calls waiting on it
                // `Unavailable` and connects again for the next.
                tracing::warn!(%error, "the MQTT link lost its connection; the calls waiting on it fail");
                return;
            }
        }
    }
}

/// Publishes one streamed request's control frames once the server has acknowledged its `open`.
async fn pump(side: Arc<ClientSide>, call: u64, opened: oneshot::Receiver<()>, mut queued: mpsc::UnboundedReceiver<Frame>) {
    if opened.await.is_err() {
        return;
    }
    // A `cancel` queued before the acknowledgment goes out alone: the call is over.
    let mut held = Vec::new();
    while let Ok(frame) = queued.try_recv() {
        held.push(frame);
    }
    if let Some(at) = held.iter().position(|frame| matches!(frame, Frame::Cancel { .. })) {
        held = vec![held.swap_remove(at)];
        queued.close();
    }
    let mut held = held.into_iter();
    loop {
        let frame = match held.next() {
            Some(frame) => frame,
            None => match queued.recv().await {
                Some(frame) => frame,
                None => break,
            },
        };
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => continue,
        };
        if let Err(error) = side.publish_control(call, kind, body).await {
            tracing::debug!(%error, kind, "the MQTT link could not publish a control frame");
        }
    }
}

fn reply_properties(correlation: Option<Bytes>, kind: Option<&str>) -> PublishProperties {
    PublishProperties {
        correlation_data: correlation,
        user_properties: kind.map(|kind| vec![(KIND.to_owned(), kind.to_owned())]).unwrap_or_default(),
        ..Default::default()
    }
}

fn user_properties(headers: &CallHeaders, kind: Option<&str>) -> Vec<(String, String)> {
    let mut properties: Vec<(String, String)> = headers.iter().map(|(name, value)| (name.to_owned(), value.to_owned())).collect();
    if let Some(kind) = kind {
        properties.push((KIND.to_owned(), kind.to_owned()));
    }
    properties
}

fn kind_of(properties: &[(String, String)]) -> Option<String> {
    properties.iter().find(|(name, _)| name == KIND).map(|(_, value)| value.clone())
}

fn call_headers(properties: &[(String, String)]) -> CallHeaders {
    let mut headers = CallHeaders::new();
    for (name, value) in properties {
        if name != KIND {
            headers.insert(name.as_str(), value.as_str());
        }
    }
    headers
}

/// What a caller receives for a miss this link signals: `Unavailable` with
/// `reason: "no_destination"`, as `RpcClient` maps a `NoDestination` from `send`.
fn no_destination(id: u64, pattern: &Pattern) -> Frame {
    let mut details = Details::new();
    details.push(Detail::ErrorInfo { reason: "no_destination".to_owned(), domain: DOMAIN.to_owned(), metadata: BTreeMap::new() });
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, format!("nothing listens on pattern `{pattern}`"), details) }
}

/// The reply frame with the caller's own `id`, read from the Correlation Data: the server wrote
/// its local id.
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

/// A client id unique to one connection, inside the 23 characters every MQTT broker accepts.
fn client_id() -> String {
    format!("ulo-{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

/// A ShareName from the root module's path: MQTT refuses `/`, `+` and `#` in one, and whitespace
/// is replaced to keep the filter readable in a broker's logs.
fn share_name(text: &str) -> String {
    text.chars().map(|c| if c.is_whitespace() || matches!(c, '/' | '+' | '#') { '_' } else { c }).collect()
}

fn check_group(group: &str) -> Result<(), BoxError> {
    if group.is_empty() || group.chars().any(|c| matches!(c, '/' | '+' | '#')) {
        return Err(format!("the MQTT link's group `{group}` is not a ShareName: it must be non-empty, with no `/`, `+` or `#`").into());
    }
    Ok(())
}

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
