use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use futures_util::StreamExt;
use lapin::message::Delivery as AmqpDelivery;
use lapin::options::{
    BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicPublishOptions, BasicQosOptions, BasicRejectOptions,
    ConfirmSelectOptions, ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::uri::AMQPUri;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties, Consumer, ExchangeKind};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use ulo::{AppHandle, BoxError, BoxFuture};
use ulo_rpc::link::Inbound;
use ulo_transport::Count;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, Frame, Link, NoDestination, Ordering as Order,
    Outbound, Pattern, ReplyPath, ReplyTo,
};

/// The header naming a message's frame kind where the message alone does not tell it: `open` on
/// a pattern's queue, `in`, `in_end` and `cancel` on the control exchange, `opened` on the reply
/// queue. A message on a pattern's queue without it is a `req` when it carries `reply_to` and an
/// `evt` when it does not.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The fanout exchange every server instance binds its own queue to, so each sees every control
/// message and acts on the ones whose `correlation_id` names a call it holds. A streamed request's
/// items and every `cancel` travel here: a pattern's queue hands each message to one consumer,
/// and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// RabbitMQ's direct reply-to pseudo-queue: the client consumes it, without acknowledgments, on
/// the channel it publishes requests on.
const REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// The per-consumer prefetch when the server's `max_inflight` is `Count::Default` or
/// `Count::Unlimited`.
const DEFAULT_PREFETCH: u16 = 64;

/// The RabbitMQ link.
pub struct RabbitMq {
    pub(crate) url: String,
    pub(crate) codec: Codec,
    pub(crate) prefetch: u16,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<Arc<ServerSide>>,
    client: Option<Connection>,
}

impl RabbitMq {
    /// The link on `url`, `amqp://..` or `amqps://..`, parsed in `prepare` and connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        RabbitMq { url: url.into(), codec: Codec::Json, prefetch: DEFAULT_PREFETCH, state: Mutex::new(State::default()) }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

}

impl Link for RabbitMq {
    const NAME: &'static str = "rabbitmq";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerQueue)
            .native_backpressure(true)
            .miss_signal(true)
            .holds_unserved(true)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        uri(&self.url)?;
        Ok(())
    }

    /// The unacknowledged deliveries each pattern's consumer takes at once (`basic.qos` with
    /// `global = false`): `Count::Max(n)` gives n, capped at AMQP's 65 535, and `Default` or
    /// `Unlimited` 64, since AMQP's own unlimited, 0, would hand an instance its whole queue.
    fn max_inflight(&mut self, calls: Count) {
        self.prefetch = match calls {
            Count::Max(calls) => u16::try_from(calls).unwrap_or(u16::MAX),
            _ => DEFAULT_PREFETCH,
        };
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let connection = Connection::connect_uri(uri(&self.url)?, ConnectionProperties::default().enable_auto_recover()).await?;
        match subscribe(&connection, patterns, self.prefetch).await {
            Ok((channel, consumers, control)) => {
                let (phase, _) = watch::channel(Phase::Serving);
                let (deliveries, inbound) = mpsc::unbounded_channel();
                let side = Arc::new(ServerSide {
                    connection,
                    channel,
                    codec: self.codec,
                    runtime: Handle::current(),
                    calls: Arc::new(Calls::new()),
                    phase,
                    tags: consumers.iter().map(Consumer::tag).collect(),
                    deliveries: Mutex::new(Some(deliveries)),
                });
                for consumer in consumers {
                    tokio::spawn(request_lane(Arc::clone(&side), consumer));
                }
                tokio::spawn(control_lane(Arc::clone(&side), control));
                lock(&self.state).server = Some(side);
                Ok(receiver_stream(inbound))
            }
            Err(error) => {
                let _ = connection.close(200, "bind failed".into()).await;
                Err(error)
            }
        }
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        // Not recovered by lapin: a direct reply-to address lives as long as its channel, so the
        // replies a recovered connection's new channel would wait for are gone. A lost connection
        // ends the reply lane instead, and `RpcClient` connects again for the next call.
        let connection = Connection::connect_uri(uri(&self.url)?, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        // Confirm mode is what makes a `mandatory` publish's `basic.return` reach the publisher:
        // lapin resolves the publish's confirmation with the returned message.
        channel.confirm_select(ConfirmSelectOptions::default()).await?;
        channel
            .exchange_declare(CONTROL.into(), ExchangeKind::Fanout, ExchangeDeclareOptions::default(), FieldTable::default())
            .await?;
        let replies = channel
            .basic_consume(
                REPLY_TO.into(),
                "".into(),
                BasicConsumeOptions { no_ack: true, ..Default::default() },
                FieldTable::default(),
            )
            .await?;

        let side = Arc::new(ClientSide {
            channel,
            codec: self.codec,
            id: uuid::Uuid::new_v4().simple().to_string(),
            calls: Mutex::new(HashMap::new()),
        });
        lock(&self.state).client = Some(connection);
        let (frames, replies_out) = mpsc::unbounded_channel();
        tokio::spawn(route_replies(Arc::clone(&side), replies, frames));

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
        for tag in &side.tags {
            if let Err(error) = side.channel.basic_cancel(tag.clone(), BasicCancelOptions::default()).await {
                tracing::warn!(%error, consumer = tag.as_str(), "the RabbitMQ link could not cancel a consumer");
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
        let mut failure = None;
        if let Some(side) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            lock(&side.deliveries).take();
            if let Err(error) = side.connection.close(200, "close".into()).await {
                failure = Some(error);
            }
        }
        if let Some(connection) = client
            && let Err(error) = connection.close(200, "close".into()).await
        {
            failure = Some(error);
        }
        match failure {
            Some(error) => Err(format!("the RabbitMQ link did not close cleanly: {error}").into()),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

fn uri(url: &str) -> Result<AMQPUri, BoxError> {
    url.parse::<AMQPUri>().map_err(|error| format!("the RabbitMQ link's url does not parse: {error}").into())
}

/// Declares a queue per pattern and consumes each under the prefetch, per consumer under
/// RabbitMQ's reading of `global = false`, then this instance's own
/// queue on the control exchange, on a channel of its own: lapin's topology replay after a
/// reconnect cannot redeclare a server-named queue, and a failed replay closes its channel.
async fn subscribe(connection: &Connection, patterns: &[Pattern], prefetch: u16) -> Result<(Channel, Vec<Consumer>, Consumer), BoxError> {
    let channel = connection.create_channel().await?;
    channel.basic_qos(prefetch, BasicQosOptions { global: false }).await?;
    let mut consumers = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        channel.queue_declare(pattern.as_str().into(), QueueDeclareOptions::default(), FieldTable::default()).await?;
        // An empty tag has the broker assign one; a fixed tag would repeat across the patterns
        // sharing the channel, which the broker refuses.
        let consumer = channel
            .basic_consume(pattern.as_str().into(), "".into(), BasicConsumeOptions::default(), FieldTable::default())
            .await?;
        consumers.push(consumer);
    }

    let control = connection.create_channel().await?;
    control
        .exchange_declare(CONTROL.into(), ExchangeKind::Fanout, ExchangeDeclareOptions::default(), FieldTable::default())
        .await?;
    let queue: ShortString = format!("{CONTROL}.{}", uuid::Uuid::new_v4().simple()).into();
    control
        .queue_declare(queue.clone(), QueueDeclareOptions { auto_delete: true, ..Default::default() }, FieldTable::default())
        .await?;
    control.queue_bind(queue.clone(), CONTROL.into(), "".into(), QueueBindOptions::default(), FieldTable::default()).await?;
    let control_consumer = control
        .basic_consume(queue, "".into(), BasicConsumeOptions { no_ack: true, ..Default::default() }, FieldTable::default())
        .await?;
    Ok((channel, consumers, control_consumer))
}

/// The server side of a bound link: the channel its pattern consumers and replies share, the
/// calls it holds, and where deliveries go.
struct ServerSide {
    connection: Connection,
    channel: Channel,
    codec: Codec,
    /// Where an `Ack`'s settlement runs: `Ack::ack` and `reject` are synchronous, and lapin's
    /// acknowledgment is a future.
    runtime: Handle,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// The pattern consumers' tags, cancelled by the drain.
    tags: Vec<ShortString>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
}

impl ServerSide {
    fn deliver(&self, delivery: Delivery) {
        if let Some(deliveries) = lock(&self.deliveries).as_ref() {
            let _ = deliveries.send(delivery);
        }
    }

    async fn on_request(&self, delivery: AmqpDelivery) {
        let pattern = delivery.routing_key.as_str().to_owned();
        let headers = delivery.properties.headers().as_ref();
        let kind = kind_of(headers);
        let call_headers = call_headers(headers);
        let reply_to = delivery.properties.reply_to().as_ref().map(|reply| reply.as_str().to_owned());
        let correlation = delivery.properties.correlation_id().as_ref().map(|id| id.as_str().to_owned());
        let acker = delivery.acker;
        let runtime = self.runtime.clone();
        // Settled once the handler completes, so the queue redelivers a call whose server died
        // mid-handler: at-least-once.
        let ack = Ack::new(move |accepted| {
            runtime.spawn(async move {
                let settled = if accepted {
                    acker.ack(BasicAckOptions::default()).await
                } else {
                    acker.reject(BasicRejectOptions { requeue: false }).await
                };
                if let Err(error) = settled {
                    tracing::warn!(%error, "the RabbitMQ link could not settle a delivery");
                }
            });
        });
        let data = Data::new(delivery.data);
        match (reply_to, kind.as_deref()) {
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
                // control exchange after the call is held here.
                if let Err(error) = publish_reply(&self.channel, reply, correlation, Some(OPENED), &[]).await {
                    tracing::warn!(%error, pattern, "the RabbitMQ link could not acknowledge a streamed request");
                }
                self.deliver(Delivery { frame: Frame::Open { id, pattern, headers: call_headers }, reply: Some(path), ack });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern, kind, "the RabbitMQ link rejected a message of an unknown kind");
                ack.reject();
            }
        }
    }

    fn on_control(&self, delivery: AmqpDelivery) {
        let Some(key) = delivery.properties.correlation_id().as_ref().map(|id| id.as_str().to_owned()) else { return };
        let frame = match kind_of(delivery.properties.headers().as_ref()).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(delivery.data) }, path)),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under `key`, its `correlation_id`, the key its control messages carry.
    fn hold(&self, key: String, reply: String, correlation: Option<String>) -> (u64, ReplyPath) {
        let channel = self.channel.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let released = key.clone();
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let channel = channel.clone();
            let calls = Arc::clone(&calls);
            let reply = reply.clone();
            let correlation = correlation.clone();
            let released = released.clone();
            Box::pin(async move {
                if is_terminal(&frame) {
                    calls.release(&released);
                }
                let bytes = codec.encode_frame(&frame)?;
                publish_reply(&channel, reply, correlation, None, &bytes).await
            })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

async fn publish_reply(channel: &Channel, reply: String, correlation: Option<String>, kind: Option<&str>, body: &[u8]) -> Result<(), BoxError> {
    let mut properties = BasicProperties::default();
    if let Some(correlation) = correlation {
        properties = properties.with_correlation_id(correlation.into());
    }
    if let Some(headers) = amqp_headers(&CallHeaders::new(), kind) {
        properties = properties.with_headers(headers);
    }
    channel.basic_publish("".into(), reply.into(), BasicPublishOptions::default(), body, properties).await?;
    Ok(())
}

/// A pattern consumer's deliveries until it is cancelled or its channel closes. lapin yields an
/// `Err` while it recovers a dropped connection and carries on after it.
async fn request_lane(side: Arc<ServerSide>, mut consumer: Consumer) {
    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => side.on_request(delivery).await,
            Err(error) => tracing::warn!(%error, "the RabbitMQ link's consumer is recovering"),
        }
    }
}

async fn control_lane(side: Arc<ServerSide>, mut consumer: Consumer) {
    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => side.on_control(delivery),
            Err(error) => tracing::warn!(%error, "the RabbitMQ link's control consumer is recovering"),
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

/// The client side: one confirm-mode channel for every publish and for the direct reply-to
/// consumer, each call's `correlation_id` `<id>.<call>`.
struct ClientSide {
    channel: Channel,
    codec: Codec,
    id: String,
    calls: Mutex<HashMap<u64, ClientCall>>,
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
                let sent = self.publish_request(&pattern, id, &headers, None, data.as_bytes()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                let mut properties = BasicProperties::default();
                if let Some(table) = amqp_headers(&headers, None) {
                    properties = properties.with_headers(table);
                }
                let confirmation = self
                    .channel
                    .basic_publish("".into(), pattern.as_str().into(), BasicPublishOptions::default(), data.as_bytes(), properties)
                    .await?
                    .await?;
                if confirmation.is_nack() {
                    return Err(format!("the RabbitMQ broker refused an event on `{pattern}`").into());
                }
                Ok(())
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { gate: Some(gate), queue });
                tokio::spawn(pump(Arc::clone(self), id, opened, queued));
                let sent = self.publish_request(&pattern, id, &headers, Some(OPEN), &[]).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
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
                    Some(ClientCall::Unary) => self.publish_control(id, CANCEL, &[]).await,
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

    /// A `mandatory` publish through the default exchange: a queue named after the pattern takes
    /// it, and with none the broker returns it, which is the miss signal.
    async fn publish_request(&self, pattern: &Pattern, call: u64, headers: &CallHeaders, kind: Option<&str>, body: &[u8]) -> Result<(), BoxError> {
        let mut properties = BasicProperties::default().with_reply_to(REPLY_TO.into()).with_correlation_id(self.correlation(call).into());
        if let Some(table) = amqp_headers(headers, kind) {
            properties = properties.with_headers(table);
        }
        let options = BasicPublishOptions { mandatory: true, ..Default::default() };
        let confirmation = self.channel.basic_publish("".into(), pattern.as_str().into(), options, body, properties).await?.await?;
        if confirmation.is_nack() {
            return Err(format!("the RabbitMQ broker refused a request on `{pattern}`").into());
        }
        if confirmation.take_message().is_some() {
            return Err(Box::new(NoDestination { pattern: pattern.to_string() }));
        }
        Ok(())
    }

    async fn publish_control(&self, call: u64, kind: &str, body: &[u8]) -> Result<(), BoxError> {
        let mut properties = BasicProperties::default().with_correlation_id(self.correlation(call).into());
        if let Some(table) = amqp_headers(&CallHeaders::new(), Some(kind)) {
            properties = properties.with_headers(table);
        }
        self.channel.basic_publish(CONTROL.into(), "".into(), BasicPublishOptions::default(), body, properties).await?.await?;
        Ok(())
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
}

/// Routes the direct reply-to deliveries until the consumer ends or fails; either ends the reply
/// lane, so `RpcClient` fails the calls waiting on it `Unavailable`.
async fn route_replies(side: Arc<ClientSide>, mut replies: Consumer, frames: mpsc::UnboundedSender<Frame>) {
    while let Some(delivery) = replies.next().await {
        let delivery = match delivery {
            Ok(delivery) => delivery,
            Err(error) => {
                tracing::warn!(%error, "the RabbitMQ link lost its reply consumer; the calls waiting on it fail");
                break;
            }
        };
        let Some(call) = delivery.properties.correlation_id().as_ref().and_then(|id| side.call_of(id.as_str())) else { continue };
        if kind_of(delivery.properties.headers().as_ref()).as_deref() == Some(OPENED) {
            side.open_gate(call);
            continue;
        }
        let frame = match side.codec.decode_frame(&delivery.data) {
            Ok(frame) => with_id(frame, call),
            Err(error) => {
                tracing::warn!(%error, "the RabbitMQ link dropped a reply that does not decode");
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
        if let Err(error) = side.publish_control(call, kind, &body).await {
            tracing::debug!(%error, kind, "the RabbitMQ link could not publish a control frame");
        }
    }
}

/// AMQP headers from a call's headers. A field table is a map, so a name repeated in the call
/// keeps its last value.
fn amqp_headers(headers: &CallHeaders, kind: Option<&str>) -> Option<FieldTable> {
    if headers.is_empty() && kind.is_none() {
        return None;
    }
    let mut table = FieldTable::default();
    for (name, value) in headers.iter() {
        table.insert(name.into(), AMQPValue::LongString(value.into()));
    }
    if let Some(kind) = kind {
        table.insert(KIND.into(), AMQPValue::LongString(kind.into()));
    }
    Some(table)
}

fn header_text(value: &AMQPValue) -> Option<String> {
    match value {
        AMQPValue::LongString(text) => Some(String::from_utf8_lossy(text.as_bytes()).into_owned()),
        AMQPValue::ShortString(text) => Some(text.as_str().to_owned()),
        AMQPValue::Boolean(value) => Some(value.to_string()),
        AMQPValue::ShortShortInt(value) => Some(value.to_string()),
        AMQPValue::ShortShortUInt(value) => Some(value.to_string()),
        AMQPValue::ShortInt(value) => Some(value.to_string()),
        AMQPValue::ShortUInt(value) => Some(value.to_string()),
        AMQPValue::LongInt(value) => Some(value.to_string()),
        AMQPValue::LongUInt(value) => Some(value.to_string()),
        AMQPValue::LongLongInt(value) => Some(value.to_string()),
        _ => None,
    }
}

fn kind_of(headers: Option<&FieldTable>) -> Option<String> {
    headers?.inner().iter().find(|(name, _)| name.as_str() == KIND).and_then(|(_, value)| header_text(value))
}

/// The call's headers from the AMQP headers: text and scalar values as text; tables, arrays and
/// byte arrays, which a header string cannot carry, are left out.
fn call_headers(headers: Option<&FieldTable>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for (name, value) in headers.inner() {
            if name.as_str() == KIND {
                continue;
            }
            if let Some(text) = header_text(value) {
                out.insert(name.as_str(), text);
            }
        }
    }
    out
}

/// The reply frame with the caller's own `id`, read from the `correlation_id`: the server wrote
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

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
