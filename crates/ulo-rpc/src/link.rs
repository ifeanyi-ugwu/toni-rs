//! The link SPI, written without macros (transports DESIGN §5.3): what each RPC transport crate
//! implements once, TCP, UDP, NATS, Redis, RabbitMQ, MQTT and Kafka.
//!
//! A link moves frames. `ulo_rpc::Server<L>` drives its server side, [`Link::listen`], and
//! [`RpcClient`](crate::RpcClient) its client side, [`Link::connect`]; routing, execution, the
//! four shapes, deadlines and cancellation are `ulo-rpc`'s. What differs per link is declared in
//! [`Capabilities`] and asserted by `ulo-rpc-conformance`.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use futures_core::stream::BoxStream;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture, Shape};
use ulo_transport::Count;

use crate::codec::Codec;
use crate::frame::Frame;

/// One RPC transport's frame carriage.
pub trait Link: Send + Sync + 'static {
    /// The link's name, `LinkInfo::name` and a `Configure` error's: `tcp`, `nats`, ...
    const NAME: &'static str;

    fn capabilities(&self) -> Capabilities;

    /// What can fail before any I/O: endpoint text, `Tls`, a broker URL. Called from
    /// `Server::prepare`, with the app, whose root module's full type path
    /// (`format!("{:#}", app.root().name())`) is the default competing-consumer group on NATS and
    /// MQTT. A client's link reports the same failures from its first `connect`.
    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        let _ = app;
        async { Ok(()) }
    }

    /// The server's in-flight bound, `Server::max_inflight` as set, called from `Server::prepare`
    /// beside [`prepare`](Self::prepare); `Count::max_inflight` reads it, 1,024 at `Default`. A
    /// link declaring `native_backpressure` stops taking requests from its broker at that bound,
    /// so they wait in the broker where the server would refuse them: AMQP's prefetch, Kafka
    /// pausing its partitions. The server frees a call's place before it settles the call's
    /// [`Ack`], so a link that counts a request in flight until its `Ack` settles, or is dropped,
    /// never hands over one past the bound. Every other link ignores it.
    fn max_inflight(&mut self, calls: Count) {
        let _ = calls;
    }

    /// Server side: subscribe to `patterns`, the mounted handlers' patterns. Called from
    /// `Server::bind`, all-or-nothing: a failure leaves nothing subscribed.
    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send;

    /// Client side: connect. `RpcClient` calls it lazily, on its first call.
    fn connect(&self) -> impl Future<Output = Result<Outbound, BoxError>> + Send;

    /// The link's own close signal, from `Server::drain`: TCP sends `goaway` on every connection,
    /// NATS drains, AMQP cancels its consumers, MQTT unsubscribes, Kafka pauses and commits. The
    /// inbound stream ends once nothing more will arrive.
    fn drain(&self) -> impl Future<Output = ()> + Send;

    /// Closes the link's connections, under the core's `close` bound.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// The addresses bound, for `App<Bound>::addresses()`, so port 0 reports the port chosen; a
    /// broker link answers none.
    fn bound(&self) -> Vec<BoundAddr> {
        Vec::new()
    }
}

/// What a link's server side delivers.
///
/// The frames' `id`s are the server's correlation: unique among the calls in flight on one
/// inbound stream, so an `in`, `in_end` or `cancel` reaches the call its `req` or `open` started.
/// A link whose callers allocate ids per connection, TCP and UDP, maps each to an id of its own
/// and back on the reply path; a broker link allocates one per native correlation.
pub type Inbound = BoxStream<'static, Delivery>;

/// One frame arriving at the server, with where its replies go and how it is acknowledged.
///
/// A link that loses the caller of calls in flight, a TCP connection closing, delivers `cancel`
/// for each with `reply: None`, which the server reads as `CancelReason::Disconnected`; a
/// `cancel` the caller sent carries its reply path and is `CancelReason::ClientCancelled`.
pub struct Delivery {
    pub frame: Frame,
    /// `None` for an event. A link may attach one to an event to carry the caller's address
    /// ([`ReplyPath::peer`]), which the server reads for `LinkInfo::peer` and never replies on.
    pub reply: Option<ReplyPath>,
    pub ack: Ack,
}

/// Where the replies to one delivery go: the connection on TCP, the sender's address on UDP, the
/// reply subject, queue, topic or channel on a broker. Cheap to clone, every clone the same path;
/// a stream's items and its end go through it one by one.
#[derive(Clone)]
pub struct ReplyPath {
    send: Arc<dyn Fn(Frame) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    peer: Option<SocketAddr>,
}

impl ReplyPath {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(Frame) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync + 'static,
    {
        ReplyPath { send: Arc::new(send), peer: None }
    }

    /// The caller's address, which the server seeds as `LinkInfo::peer`: TCP and UDP set it.
    pub fn peer(self, peer: SocketAddr) -> Self {
        ReplyPath { peer: Some(peer), ..self }
    }

    /// Sends one reply frame. A send that fails with [`FrameTooLarge`] or [`FrameUnencodable`] is
    /// answered by the server with an `err` frame of kind `internal` under the same `id`, the
    /// second logged at `error`; any other failure means the caller is gone, and the call is
    /// cancelled `Disconnected`. A link encoding with [`Codec::encode_frame`] and passing its
    /// error on with `?` reports an encoding failure as `FrameUnencodable`.
    pub fn send(&self, frame: Frame) -> BoxFuture<'static, Result<(), BoxError>> {
        (self.send)(frame)
    }

    pub(crate) fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }
}

impl fmt::Debug for ReplyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplyPath")
    }
}

/// How one delivery is settled with the broker, once its handler completes: acknowledged, or
/// rejected without requeue (an unhandled event, so it cannot loop on redelivery). A no-op off
/// AMQP and Kafka. Dropped unsettled it sends nothing, and the broker redelivers the message once
/// the consumer's channel closes.
pub struct Ack {
    settle: Mutex<Option<Box<dyn FnOnce(bool) + Send>>>,
}

impl Ack {
    /// An acknowledgment that `settle` performs, `true` for `ack` and `false` for `reject`.
    pub fn new(settle: impl FnOnce(bool) + Send + 'static) -> Self {
        Ack { settle: Mutex::new(Some(Box::new(settle))) }
    }

    /// An acknowledgment with nothing to do, for a link whose broker acknowledges nothing.
    pub fn none() -> Self {
        Ack { settle: Mutex::new(None) }
    }

    pub fn ack(self) {
        self.settle(true);
    }

    /// AMQP `basic.reject` with `requeue = false`, which routes to a dead-letter exchange when one
    /// is configured; Kafka commits the offset.
    pub fn reject(self) {
        self.settle(false);
    }

    fn settle(&self, accepted: bool) {
        let settle = self.settle.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(settle) = settle {
            settle(accepted);
        }
    }
}

impl fmt::Debug for Ack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ack")
    }
}

/// A link's client side, from [`Link::connect`]: `send` publishes a frame on `pattern`, with the
/// reply's correlation when one is expected, and `replies` yields every reply-lane frame for this
/// client. A link with a miss signal fails `send` with [`NoDestination`]; one refusing a frame over
/// its size with [`FrameTooLarge`].
pub struct Outbound {
    pub send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    pub replies: BoxStream<'static, Frame>,
}

/// A request's reply correlation, which the link maps onto its own mechanism: the frame's `id` on
/// TCP and UDP, an `_INBOX` subject, `correlation_id`, Correlation Data, a reply header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReplyTo {
    pub id: u64,
}

/// A pattern, as a handler declares it and a call names it: the subject, channel, queue or topic
/// on a broker.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pattern(Arc<str>);

impl Pattern {
    pub fn new(pattern: impl Into<Arc<str>>) -> Self {
        Pattern(pattern.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Pattern {
    fn from(pattern: &str) -> Self {
        Pattern(Arc::from(pattern))
    }
}

impl From<String> for Pattern {
    fn from(pattern: String) -> Self {
        Pattern(Arc::from(pattern))
    }
}

/// What a link can do, read by `Server::prepare` and by the client, and asserted by the
/// conformance suite in both directions. Built from [`Capabilities::new`] and its setters, since
/// the struct is `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The codec carries raw bytes.
    pub binary: bool,
    /// The largest frame the link carries, `None` for no limit of its own.
    pub max_frame: Option<u64>,
    pub ordering: Ordering,
    /// The call shapes the link carries; a handler of another shape is refused in `prepare`.
    pub shapes: &'static [Shape],
    /// The broker paces deliveries itself, AMQP through its prefetch and Kafka by pausing
    /// partitions: over `Server::max_inflight` the link stops taking requests, which wait in the
    /// broker. On a link without it the server refuses a request over the bound `unavailable`
    /// with a `RetryAfter` detail.
    pub native_backpressure: bool,
    pub delivery: DeliveryMode,
    /// A request to a pattern nothing subscribes to is reported to the caller, which then answers
    /// `Unavailable` with `reason: "no_destination"`; without it the caller's own `Timeout`.
    pub miss_signal: bool,
    /// The broker keeps a request for a pattern a server has subscribed while no instance consumes
    /// it, and hands it to the next instance: an AMQP queue, a Kafka topic. A caller reaching a
    /// draining or stopped server then sees its own `Timeout`, and `miss_signal` covers only a
    /// pattern never subscribed.
    pub holds_unserved: bool,
    /// The broker keeps a reply published while the client's connection is down, and the client
    /// reads it once it reconnects: a Kafka reply topic. A call waiting when the client loses its
    /// connection is then answered, rather than failed `Unavailable` as on a link whose replies
    /// the outage loses.
    pub durable_replies: bool,
    /// The link's drain returns only once nothing more is on its way to the server, so every
    /// request that reached it is answered, `Unavailable` once it is draining. `true` unless a link
    /// declares otherwise, so a link that does not declare it is held to the confirmed drain. A link
    /// declaring `false` cannot confirm it, and a request already on its way can be lost: the NATS
    /// client removes a drained subscription before the server has processed the unsubscribe and
    /// discards what arrives for it afterwards, and a datagram reaching a UDP server after its
    /// socket closes is dropped. A caller reaching a draining server can then see its own `Timeout`
    /// rather than `Unavailable`; once the server has closed, a miss is reported as on any link with
    /// `miss_signal`.
    pub confirms_drain: bool,
}

/// Every call shape, for a link that carries them all.
pub const ALL_SHAPES: &[Shape] = &[Shape::Unary, Shape::ServerStreaming, Shape::ClientStreaming, Shape::Bidi];

/// Unary calls and events alone, UDP's.
pub const UNARY_ONLY: &[Shape] = &[Shape::Unary];

impl Capabilities {
    /// A link delivering as `delivery`, carrying every shape, JSON, unordered, with no frame limit,
    /// no native backpressure, no miss signal, nothing held for a server that has gone, no reply
    /// kept through a client's outage, and a confirmed drain: `confirms_drain` is the one
    /// capability `new` sets `true`.
    pub const fn new(delivery: DeliveryMode) -> Self {
        Capabilities {
            binary: false,
            max_frame: None,
            ordering: Ordering::Unordered,
            shapes: ALL_SHAPES,
            native_backpressure: false,
            delivery,
            miss_signal: false,
            holds_unserved: false,
            durable_replies: false,
            confirms_drain: true,
        }
    }

    pub const fn binary(self, binary: bool) -> Self {
        Capabilities { binary, ..self }
    }

    pub const fn max_frame(self, max_frame: Option<u64>) -> Self {
        Capabilities { max_frame, ..self }
    }

    pub const fn ordering(self, ordering: Ordering) -> Self {
        Capabilities { ordering, ..self }
    }

    pub const fn shapes(self, shapes: &'static [Shape]) -> Self {
        Capabilities { shapes, ..self }
    }

    pub const fn native_backpressure(self, native_backpressure: bool) -> Self {
        Capabilities { native_backpressure, ..self }
    }

    pub const fn miss_signal(self, miss_signal: bool) -> Self {
        Capabilities { miss_signal, ..self }
    }

    pub const fn holds_unserved(self, holds_unserved: bool) -> Self {
        Capabilities { holds_unserved, ..self }
    }

    pub const fn durable_replies(self, durable_replies: bool) -> Self {
        Capabilities { durable_replies, ..self }
    }

    pub const fn confirms_drain(self, confirms_drain: bool) -> Self {
        Capabilities { confirms_drain, ..self }
    }
}

/// How a request reaches two server instances listening on one pattern.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeliveryMode {
    /// One instance takes each request: AMQP queues, Kafka consumer groups, a NATS queue group, an
    /// MQTT shared subscription.
    Competing,
    /// Every instance receives each request: Redis Pub/Sub; the client drops a second reply.
    FanOut,
    /// The caller names the server: TCP and UDP.
    Addressed,
}

/// The order a link keeps.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ordering {
    /// No order: UDP.
    Unordered,
    /// Per connection: TCP.
    PerConnection,
    /// Per publisher and subject: NATS.
    PerPublisher,
    /// Per channel: Redis.
    PerChannel,
    /// Per queue with a single consumer: AMQP.
    PerQueue,
    /// Per topic and QoS: MQTT.
    PerTopic,
    /// Per partition: Kafka.
    PerPartition,
}

/// A link's report that nothing listens on the pattern a request named: NATS no-responders, a
/// Redis receiver count of zero, an AMQP `basic.return`, an MQTT PUBACK or PUBREC with reason
/// 0x10. `RpcClient` answers it `Unavailable` with `reason: "no_destination"`.
#[derive(Debug)]
pub struct NoDestination {
    pub pattern: String,
}

impl fmt::Display for NoDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "nothing listens on pattern `{}`", self.pattern)
    }
}

impl Error for NoDestination {}

/// A frame over the link's limit, refused before it is sent. `RpcClient` answers it `BadRequest`
/// with `reason: "payload_too_large"`.
#[derive(Debug)]
pub struct FrameTooLarge {
    pub size: u64,
    pub limit: u64,
}

impl fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a frame of {} bytes is over the link's limit of {} bytes", self.size, self.limit)
    }
}

impl Error for FrameTooLarge {}

/// A frame the link's codec cannot encode, from [`Codec::encode_frame`]: a payload that is not an
/// item of that codec, such as a `Data` built from JSON bytes and answered on a CBOR link. On a
/// reply lane it is the server's own failure, which the server answers with an `err` of kind
/// `internal` and logs at `error`; `RpcClient` answers it `Internal` on a request.
#[derive(Debug)]
pub struct FrameUnencodable {
    pub codec: Codec,
    pub source: BoxError,
}

impl fmt::Display for FrameUnencodable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {:?} codec cannot encode the frame: {}", self.codec, self.source)
    }
}

impl Error for FrameUnencodable {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}
