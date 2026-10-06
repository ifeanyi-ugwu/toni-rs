use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use rumqttc::v5::mqttbytes::QoS as MqttQoS;
use rumqttc::v5::mqttbytes::v5::{Filter, Packet, PubAckReason, PubRecReason, Publish, PublishProperties, SubscribeReasonCode};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use rumqttc::{Outgoing, Transport};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use ulo::{AppHandle, BoxError, BoxFuture};
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

/// The MQTT v5 link.
pub struct Mqtt {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) qos: QoS,
    pub(crate) codec: Codec,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The CONNACK's Maximum Packet Size, zero until a connection reads one.
    pub(crate) max_packet: Arc<AtomicU64>,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, AbortHandle)>,
    client: Option<(AsyncClient, AbortHandle)>,
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
}

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

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
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
        let target = Target::parse(&self.url)?;
        let (client, eventloop) = AsyncClient::new(target.options(client_id()), 64);
        let shared: Vec<String> = patterns.iter().map(|pattern| format!("$share/{group}/{pattern}")).collect();
        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let side = Arc::new(ServerSide {
            client,
            codec: self.codec,
            qos: self.qos.wire(),
            shared,
            calls: Arc::new(Calls::new()),
            phase,
            max_packet: Arc::clone(&self.max_packet),
            deliveries: Mutex::new(Some(deliveries)),
        });

        let (ready, bound) = oneshot::channel();
        let task = tokio::spawn(server_loop(Arc::clone(&side), eventloop, ready));
        // `bind`'s outcome waits for the first CONNACK and the SUBACK of every subscription, so a
        // broker without shared subscriptions, or one refusing a filter, fails `bind`.
        let outcome = bound.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        if let Err(error) = outcome {
            task.abort();
            let _ = side.client.try_disconnect();
            return Err(error);
        }
        lock(&self.state).server = Some((side, task.abort_handle()));
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let target = Target::parse(&self.url)?;
        let id = client_id();
        let reply_topic = format!("ulo/rpc/reply/{id}");
        let (client, eventloop) = AsyncClient::new(target.options(id.clone()), 64);
        let side = Arc::new(ClientSide {
            client: client.clone(),
            codec: self.codec,
            qos: self.qos.wire(),
            id,
            reply_topic,
            max_packet: Arc::clone(&self.max_packet),
            calls: Mutex::new(HashMap::new()),
            order: tokio::sync::Mutex::new(()),
            outgoing: Mutex::new(None),
            pkids: Mutex::new(HashMap::new()),
        });
        let (frames, replies) = mpsc::unbounded_channel();
        let (ready, subscribed) = oneshot::channel();
        let task = tokio::spawn(client_loop(Arc::clone(&side), eventloop, frames, ready));
        // The reply subscription is in place before the first request can be published.
        let outcome = subscribed.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        if let Err(error) = outcome {
            task.abort();
            return Err(error);
        }
        lock(&self.state).client = Some((client, task.abort_handle()));

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
            if let Err(error) = side.client.unsubscribe(filter.clone()).await {
                tracing::warn!(%error, filter, "the MQTT link could not unsubscribe");
            }
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
        if let Some((side, task)) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            lock(&side.deliveries).take();
            let _ = side.client.disconnect().await;
            task.abort();
        }
        if let Some((client, task)) = client {
            let _ = client.disconnect().await;
            task.abort();
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
    /// The `$share/<group>/<pattern>` filters, unsubscribed by the drain.
    shared: Vec<String>,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    max_packet: Arc<AtomicU64>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
}

impl ServerSide {
    fn deliver(&self, delivery: Delivery) {
        if let Some(deliveries) = lock(&self.deliveries).as_ref() {
            let _ = deliveries.send(delivery);
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
        if topic == CONTROL {
            let Some(key) = key else { return };
            let frame = match kind.as_deref() {
                Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(publish.payload) }, path)),
                Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
                Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
                _ => None,
            };
            if let Some((frame, path)) = frame {
                self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
            }
            return;
        }

        let headers = call_headers(&properties.user_properties);
        let reply = properties.response_topic.clone().map(|topic| (topic, properties.correlation_data.clone()));
        match (reply, kind.as_deref()) {
            (None, None) => {
                let frame = Frame::Evt { pattern: topic, headers, data: Data::new(publish.payload) };
                self.deliver(Delivery { frame, reply: None, ack: Ack::none() });
            }
            (Some((reply, correlation)), None) => {
                let (id, path) = self.hold(key.unwrap_or_else(|| reply.clone()), reply, correlation);
                let frame = Frame::Req { id, pattern: topic, headers, data: Data::new(publish.payload) };
                self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
            }
            (Some((reply, correlation)), Some(OPEN)) => {
                let (id, path) = self.hold(key.unwrap_or_else(|| reply.clone()), reply.clone(), correlation.clone());
                // The caller holds the request's items until this arrives. Queued without waiting:
                // this runs on the event loop, which is what drains the queue.
                let opened = reply_properties(correlation, Some(OPENED));
                if let Err(error) = self.client.try_publish_with_properties(reply, self.qos, false, Bytes::new(), opened) {
                    tracing::warn!(%error, pattern = topic, "the MQTT link could not acknowledge a streamed request");
                }
                self.deliver(Delivery { frame: Frame::Open { id, pattern: topic, headers }, reply: Some(path), ack: Ack::none() });
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

/// Polls the server connection until the link closes, which aborts it. On every CONNACK it
/// subscribes again, rumqttc keeping no subscription across a reconnect: the shared filters while
/// serving, the control topic always.
async fn server_loop(side: Arc<ServerSide>, mut eventloop: EventLoop, ready: oneshot::Sender<Result<(), BoxError>>) {
    let mut ready = Some(ready);
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(connack))) => {
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
            Ok(_) => {}
            Err(error) => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
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
    id: String,
    reply_topic: String,
    max_packet: Arc<AtomicU64>,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Held across one publish and its `Outgoing::Publish` event, so the event names that
    /// publish's packet id: rumqttc's `publish` returns before the id is assigned.
    order: tokio::sync::Mutex<()>,
    /// The waiting publish's packet id, `None` when the connection failed first.
    outgoing: Mutex<Option<oneshot::Sender<Option<u16>>>>,
    /// The packet id of each request publish awaiting its PUBACK or PUBREC, and its call.
    pkids: Mutex<HashMap<u16, u64>>,
}

enum ClientCall {
    Unary { pattern: Pattern },
    /// A streamed request: its `in`, `in_end` and `cancel` frames wait in `queue` until the server
    /// acknowledges the `open`, then go out in order.
    Streaming { pattern: Pattern, gate: Option<oneshot::Sender<()>>, queue: mpsc::UnboundedSender<Frame> },
}

impl ClientCall {
    fn pattern(&self) -> &Pattern {
        match self {
            ClientCall::Unary { pattern } | ClientCall::Streaming { pattern, .. } => pattern,
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
                tokio::spawn(pump(Arc::clone(self), id, opened, queued));
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
                let call = lock(&self.calls).remove(&id);
                match call {
                    Some(ClientCall::Unary { .. }) => self.publish_control(id, CANCEL, Bytes::new()).await,
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
        *lock(&self.outgoing) = Some(assigned);
        if let Err(error) = self.client.publish_with_properties(topic, self.qos, false, body, properties).await {
            lock(&self.outgoing).take();
            return Err(format!("the MQTT link could not queue a publish: {error}").into());
        }
        match pkid.await {
            Ok(Some(pkid)) => {
                if let Some(call) = call
                    && pkid != 0
                {
                    lock(&self.pkids).insert(pkid, call);
                }
                Ok(())
            }
            _ => Err("the MQTT link lost its connection before the publish went out".into()),
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
        if let Some(ClientCall::Streaming { gate, .. }) = lock(&self.calls).get_mut(&id)
            && let Some(gate) = gate.take()
        {
            let _ = gate.send(());
        }
    }

    fn take(&self, id: u64) -> Option<ClientCall> {
        lock(&self.calls).remove(&id)
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

/// Polls the client connection until the link closes, which aborts it.
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
                if let Some(assigned) = lock(&side.outgoing).take() {
                    let _ = assigned.send(Some(pkid));
                }
            }
            Ok(_) => {}
            Err(error) => {
                // A clean session drops whatever was queued, so the waiting publish is lost and no
                // acknowledgment will come for the packets in flight.
                if let Some(assigned) = lock(&side.outgoing).take() {
                    let _ = assigned.send(None);
                }
                lock(&side.pkids).clear();
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
                    return;
                }
                tracing::warn!(%error, "the MQTT link lost its connection; reconnecting");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
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
