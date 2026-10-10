use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use redis::aio::{ConnectionManager, PubSubSink, PubSubStream};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use ulo::{AppHandle, BoxError, BoxFuture};
use ulo_tokio::Tokio;
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, Link, NoDestination, Ordering as Order,
    Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::{Details, ErrorKind};

/// The reserved header a `req` or `open` frame carries in its `h`: the caller's reply channel.
/// Redis Pub/Sub has no reply address of its own, so the whole frame carries it, and the server
/// removes it before the call's headers reach a handler.
const REPLY: &str = "ulo-reply";

/// The channel every server subscribes to beside its patterns, carrying `in`, `in_end` and
/// `cancel` frames. A server acts on the ones whose `id` names a call it holds, so a drained
/// server that unsubscribed its patterns still receives the items of a streamed request it holds.
const CONTROL: &str = "ulo:rpc:control";

/// The Redis link.
///
/// The link's connections and tasks live on the tokio runtime it holds, the one current where it
/// was built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may
/// be polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `prepare` and in `connect`.
pub struct Redis {
    pub(crate) url: String,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, AbortHandle)>,
    client: Option<(Arc<ClientSide>, AbortHandle)>,
}

impl Redis {
    /// The link on `url`, `redis://host:6379` or `rediss://host:6380`, parsed in `prepare` and
    /// connected lazily. `rediss://` needs the crate's `tls` feature.
    pub fn url(url: impl Into<String>) -> Self {
        Redis { url: url.into(), codec: Codec::Json, runtime: Tokio::try_current(), state: Mutex::new(State::default()) }
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

const NO_RUNTIME: &str = "the Redis link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Redis {
    const NAME: &'static str = "redis";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::FanOut).binary(self.codec.binary()).ordering(Order::PerChannel).miss_signal(true)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        self.runtime()?;
        client(&self.url)?;
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let runtime = self.runtime()?;
        let client = client(&self.url)?;
        let patterns: Vec<String> = patterns.iter().map(|pattern| pattern.as_str().to_owned()).collect();
        let (publisher, sink, stream) = {
            let (client, channels) = (client.clone(), server_channels(&patterns, true));
            runtime
                .run(async move {
                    let publisher = ConnectionManager::new(client.clone()).await?;
                    // Subscribed before `listen` returns, so a failure leaves nothing subscribed
                    // and the first call after `bind` finds the server listening.
                    let (sink, stream) = subscribe(&client, &channels).await?;
                    Ok::<_, BoxError>((publisher, sink, stream))
                })
                .await??
        };

        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let side = Arc::new(ServerSide {
            client,
            publisher,
            codec: self.codec,
            runtime: runtime.clone(),
            patterns,
            sink: Mutex::new(Some(sink)),
            calls: Arc::new(Calls::new()),
            phase,
            deliveries: Mutex::new(Some(deliveries)),
        });
        let task = runtime.handle().spawn(server_lane(Arc::clone(&side), stream));
        lock(&self.state).server = Some((side, task.abort_handle()));
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let client = client(&self.url)?;
        let channel = format!("ulo:rpc:reply:{}", uuid::Uuid::new_v4().simple());
        let (publisher, sink, stream) = {
            let channel = channel.clone();
            runtime
                .run(async move {
                    let publisher = ConnectionManager::new(client.clone()).await?;
                    let (sink, stream) = subscribe(&client, std::slice::from_ref(&channel)).await?;
                    Ok::<_, BoxError>((publisher, sink, stream))
                })
                .await??
        };
        let side = Arc::new(ClientSide {
            publisher: Mutex::new(Some(publisher)),
            codec: self.codec,
            channel,
            // Each call's wire id is this base plus its id, so two callers counting from zero do
            // not collide on the servers' control channel, and a reply's id gives back the call's.
            base: uuid::Uuid::new_v4().as_u64_pair().0,
        });
        let (frames, replies) = mpsc::unbounded_channel();
        let task = runtime.handle().spawn(client_lane(Arc::clone(&side), sink, stream, frames));
        lock(&self.state).client = Some((Arc::clone(&side), task.abort_handle()));

        Ok(Outbound {
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let side = Arc::clone(&side);
                let sent = runtime.run(async move { side.send(pattern, frame).await });
                Box::pin(async move { sent.await? })
            }),
            replies: receiver_stream(replies),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.as_ref().map(|(side, _)| Arc::clone(side));
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        let sink = lock(&side.sink).clone();
        if let Some(mut sink) = sink
            && !side.patterns.is_empty()
        {
            let patterns = side.patterns.clone();
            let unsubscribed = side.runtime.run(async move { sink.unsubscribe(&patterns).await.map_err(BoxError::from) }).await;
            if let Err(error) = unsubscribed.unwrap_or_else(|stopped| Err(stopped.into())) {
                tracing::warn!(%error, "the Redis link could not unsubscribe its patterns");
            }
        }
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
            lock(&side.sink).take();
            task.abort();
        }
        if let Some((side, task)) = client {
            // The `RpcClient` keeps the connection's send half until its next call replaces it,
            // so the publisher is taken out of it here, closing its connection.
            lock(&side.publisher).take();
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

fn client(url: &str) -> Result<redis::Client, BoxError> {
    redis::Client::open(url).map_err(|error| format!("the Redis link's url does not parse: {error}").into())
}

/// A Pub/Sub connection subscribed to `channels`.
async fn subscribe(client: &redis::Client, channels: &[String]) -> Result<(PubSubSink, PubSubStream), BoxError> {
    let (mut sink, stream) = client.get_async_pubsub().await?.split();
    if !channels.is_empty() {
        sink.subscribe(channels).await?;
    }
    Ok((sink, stream))
}

/// Reads the server's Pub/Sub connection until the link closes, which aborts it. Redis Pub/Sub
/// has no recovery of its own: when the connection drops the stream ends, and this reconnects
/// and subscribes again, the patterns only while serving.
async fn server_lane(side: Arc<ServerSide>, mut stream: PubSubStream) {
    loop {
        while let Some(message) = stream.next().await {
            let channel = message.get_channel_name().to_owned();
            side.on_message(&channel, message.get_payload_bytes()).await;
        }
        if *side.phase.borrow() == Phase::Closed {
            return;
        }
        tracing::warn!("the Redis link lost its Pub/Sub connection; reconnecting");
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let serving = *side.phase.borrow() == Phase::Serving;
            match subscribe(&side.client, &server_channels(&side.patterns, serving)).await {
                Ok((sink, next)) => {
                    *lock(&side.sink) = Some(sink);
                    stream = next;
                    break;
                }
                Err(error) => tracing::debug!(%error, "the Redis link could not reconnect yet"),
            }
        }
    }
}

/// The channels a server subscribes to: its patterns while serving, the control channel always.
fn server_channels(patterns: &[String], serving: bool) -> Vec<String> {
    let mut channels = if serving { patterns.to_vec() } else { Vec::new() };
    channels.push(CONTROL.to_owned());
    channels
}

/// The server side of a bound link: the publisher replies go through, the Pub/Sub sink the drain
/// unsubscribes on, and the calls it holds.
struct ServerSide {
    client: redis::Client,
    publisher: ConnectionManager,
    codec: Codec,
    /// The link's runtime, which every reply is published from.
    runtime: Tokio,
    patterns: Vec<String>,
    sink: Mutex<Option<PubSubSink>>,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream. Its lock is
    /// held while a call is held and handed over, and while the drain's watcher checks that no
    /// call is held and takes it, so the stream never ends between the two.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
}

impl ServerSide {
    /// Ends the inbound stream if no call is held, answering whether it has ended.
    fn end_if_idle(&self) -> bool {
        let mut deliveries = lock(&self.deliveries);
        if !self.calls.is_empty() {
            return false;
        }
        deliveries.take();
        true
    }

    async fn on_message(&self, channel: &str, payload: &[u8]) {
        let frame = match self.codec.decode_frame(payload) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, channel, "the Redis link dropped a message that does not decode");
                return;
            }
        };
        if let Some((wire, reply)) = self.accept(channel, frame) {
            self.refuse(wire, &reply, channel).await;
        }
    }

    /// Hands `frame` to the server while the inbound stream takes it. A request or streamed
    /// request it no longer takes comes back as its wire id and reply channel, to be refused.
    fn accept(&self, channel: &str, frame: Frame) -> Option<(u64, String)> {
        let deliveries = lock(&self.deliveries);
        if channel == CONTROL {
            let sender = deliveries.as_ref()?;
            let delivery = match frame {
                Frame::In { id, data } => self.calls.get(&id.to_string()).map(|(local, path)| (Frame::In { id: local, data }, path)),
                Frame::InEnd { id } => self.calls.get(&id.to_string()).map(|(local, path)| (Frame::InEnd { id: local }, path)),
                Frame::Cancel { id } => self.calls.release(&id.to_string()).map(|(local, path)| (Frame::Cancel { id: local }, path)),
                _ => None,
            };
            if let Some((frame, path)) = delivery {
                let _ = sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
            }
            return None;
        }
        match frame {
            Frame::Evt { headers, data, .. } => {
                if let Some(sender) = deliveries.as_ref() {
                    let frame = Frame::Evt { pattern: channel.to_owned(), headers, data };
                    let _ = sender.send(Delivery { frame, reply: None, ack: Ack::none() });
                }
                None
            }
            Frame::Req { id, headers, data, .. } => {
                let (headers, reply) = split_reply(headers);
                let Some(reply) = reply else {
                    tracing::warn!(channel, "the Redis link dropped a request naming no reply channel");
                    return None;
                };
                let Some(sender) = deliveries.as_ref() else { return Some((id, reply)) };
                let (local, path) = self.hold(id, reply.clone());
                let frame = Frame::Req { id: local, pattern: channel.to_owned(), headers, data };
                if sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() }).is_ok() {
                    return None;
                }
                self.calls.release(&id.to_string());
                Some((id, reply))
            }
            Frame::Open { id, headers, .. } => {
                let (headers, reply) = split_reply(headers);
                let Some(reply) = reply else {
                    tracing::warn!(channel, "the Redis link dropped a streamed request naming no reply channel");
                    return None;
                };
                let Some(sender) = deliveries.as_ref() else { return Some((id, reply)) };
                let (local, path) = self.hold(id, reply.clone());
                let frame = Frame::Open { id: local, pattern: channel.to_owned(), headers };
                if sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() }).is_ok() {
                    return None;
                }
                self.calls.release(&id.to_string());
                Some((id, reply))
            }
            other => {
                tracing::warn!(channel, kind = other.kind(), "the Redis link dropped a frame a pattern's channel does not carry");
                None
            }
        }
    }

    /// Answers a request or streamed request that arrived once the inbound stream ended `err` of
    /// kind `unavailable`, as the server answers one during the drain. Redis delivers what it
    /// published before processing the drain's UNSUBSCRIBE, and the server lane may read it after
    /// the stream ended; dropping it would leave the caller to its own `Timeout`.
    async fn refuse(&self, wire: u64, reply: &str, channel: &str) {
        let error = ErrorBody::new(ErrorKind::Unavailable, "the server is shutting down", Details::new());
        let mut publisher = self.publisher.clone();
        let refused = match self.codec.encode_frame(&Frame::Err { id: wire, error }) {
            Ok(bytes) => publish(&mut publisher, reply, &bytes).await.map(|_| ()),
            Err(error) => Err(BoxError::from(error)),
        };
        if let Err(error) = refused {
            tracing::warn!(%error, channel, "the Redis link could not refuse a request that arrived after the drain");
        }
    }

    /// Holds a call under its wire id, the key its control frames carry. Each reply goes out
    /// under that id, from which the caller recovers its own.
    fn hold(&self, wire: u64, reply: String) -> (u64, ReplyPath) {
        let publisher = self.publisher.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let runtime = self.runtime.clone();
        let key = wire.to_string();
        let released = key.clone();
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let mut publisher = publisher.clone();
            let calls = Arc::clone(&calls);
            let reply = reply.clone();
            let released = released.clone();
            let sent = runtime.run(async move {
                if is_terminal(&frame) {
                    calls.release(&released);
                }
                let bytes = codec.encode_frame(&with_id(frame, wire))?;
                publish(&mut publisher, &reply, &bytes).await?;
                Ok::<_, BoxError>(())
            });
            Box::pin(async move { sent.await? })
        });
        let local = self.calls.hold(key, path.clone());
        (local, path)
    }
}

/// The calls a server holds, by the wire id their control frames carry, each with its local id
/// and its reply path. The local id is this link's own, unique across every caller.
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

/// The client side: one reply channel for every reply. Every server subscribed to a pattern
/// answers, so a reply may arrive twice; `RpcClient` drops the second.
struct ClientSide {
    /// `None` once the link has closed.
    publisher: Mutex<Option<ConnectionManager>>,
    codec: Codec,
    channel: String,
    base: u64,
}

impl ClientSide {
    async fn send(&self, pattern: Pattern, frame: Frame) -> Result<(), BoxError> {
        let mut publisher = lock(&self.publisher).clone().ok_or("the Redis link's client is closed")?;
        match frame {
            Frame::Req { id, pattern: named, headers, data } => {
                let frame = Frame::Req { id: self.wire(id), pattern: named, headers: self.with_reply(headers), data };
                self.request(&mut publisher, &pattern, &frame).await
            }
            Frame::Open { id, pattern: named, headers } => {
                let frame = Frame::Open { id: self.wire(id), pattern: named, headers: self.with_reply(headers) };
                self.request(&mut publisher, &pattern, &frame).await
            }
            evt @ Frame::Evt { .. } => {
                publish(&mut publisher, pattern.as_str(), &self.codec.encode_frame(&evt)?).await?;
                Ok(())
            }
            Frame::In { id, data } => self.control(&mut publisher, &Frame::In { id: self.wire(id), data }).await,
            Frame::InEnd { id } => self.control(&mut publisher, &Frame::InEnd { id: self.wire(id) }).await,
            Frame::Cancel { id } => self.control(&mut publisher, &Frame::Cancel { id: self.wire(id) }).await,
            other => Err(format!("a client does not send a `{}` frame", other.kind()).into()),
        }
    }

    fn wire(&self, id: u64) -> u64 {
        self.base.wrapping_add(id)
    }

    fn with_reply(&self, mut headers: CallHeaders) -> CallHeaders {
        headers.insert(REPLY, self.channel.as_str());
        headers
    }

    /// The `PUBLISH` reply counts the subscribers that received the message; zero is the miss
    /// signal.
    async fn request(&self, publisher: &mut ConnectionManager, pattern: &Pattern, frame: &Frame) -> Result<(), BoxError> {
        let receivers = publish(publisher, pattern.as_str(), &self.codec.encode_frame(frame)?).await?;
        if receivers == 0 {
            return Err(Box::new(NoDestination { pattern: pattern.to_string() }));
        }
        Ok(())
    }

    async fn control(&self, publisher: &mut ConnectionManager, frame: &Frame) -> Result<(), BoxError> {
        publish(publisher, CONTROL, &self.codec.encode_frame(frame)?).await?;
        Ok(())
    }
}

/// Reads the client's reply channel until the link closes, which aborts it, or the Pub/Sub
/// connection drops. A drop ends the reply lane: what was published to the channel while it was
/// down is gone, so `RpcClient` fails the calls waiting on it `Unavailable` and connects again for
/// the next.
async fn client_lane(side: Arc<ClientSide>, sink: PubSubSink, mut stream: PubSubStream, frames: mpsc::UnboundedSender<Frame>) {
    // Dropping the sink ends the subscription, so it lives as long as the stream it pairs with.
    let _sink = sink;
    while let Some(message) = stream.next().await {
        let frame = match side.codec.decode_frame(message.get_payload_bytes()) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "the Redis link dropped a reply that does not decode");
                continue;
            }
        };
        let Some(wire) = frame.id() else { continue };
        if frames.send(with_id(frame, wire.wrapping_sub(side.base))).is_err() {
            return;
        }
    }
    tracing::warn!("the Redis link lost its reply subscription; the calls waiting on it fail");
}

async fn publish(publisher: &mut ConnectionManager, channel: &str, bytes: &[u8]) -> Result<usize, BoxError> {
    Ok(redis::cmd("PUBLISH").arg(channel).arg(bytes).query_async::<usize>(publisher).await?)
}

/// The call's headers without the reserved reply channel, and the channel.
fn split_reply(headers: CallHeaders) -> (CallHeaders, Option<String>) {
    let mut kept = CallHeaders::new();
    let mut reply = None;
    for (name, value) in headers.iter() {
        if name == REPLY {
            reply.get_or_insert_with(|| value.to_owned());
        } else {
            kept.insert(name, value);
        }
    }
    (kept, reply)
}

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
