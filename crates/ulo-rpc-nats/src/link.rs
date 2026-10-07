use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_nats::connection::State as Connection;
use async_nats::{Client, ConnectOptions, Event, HeaderMap, Message, ServerAddr, StatusCode, Subscriber};
use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use ulo::{AppHandle, BoxError, BoxFuture};
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::{Detail, Details, ErrorKind};

/// The header naming a message's frame kind where the message alone does not tell it: `open` on
/// the request lane, `in`, `in_end` and `cancel` on the control lane, `opened` on the reply lane.
/// A request-lane message without it is a `req` when it carries a reply subject and an `evt` when
/// it does not, so a body published from the NATS CLI reaches a handler.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The subject every server instance subscribes to without a queue group, so each sees every
/// control message and acts on the ones whose reply subject names a call it holds. A streamed
/// request's items and every `cancel` travel here: the request lane hands each message to one
/// instance of the group, and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// The `ErrorInfo` domain of the `no_destination` error this link writes for no-responders.
const DOMAIN: &str = "ulo.rpc";

/// The NATS link.
pub struct Nats {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) codec: Codec,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The server's `max_payload` from its `INFO`, zero until a connection reads it.
    pub(crate) max_payload: Arc<AtomicU64>,
    pub(crate) state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<Arc<ServerSide>>,
    client: Option<Client>,
}

impl Nats {
    /// The link on `url`, `nats://host:4222` or `tls://host:4222`, parsed in `prepare` and
    /// connected lazily: the server side at `bind`, the client side on its first call. Several
    /// servers are written comma-separated.
    pub fn url(url: impl Into<String>) -> Self {
        Nats {
            url: url.into(),
            group: None,
            codec: Codec::Json,
            default_group: None,
            max_payload: Arc::new(AtomicU64::new(0)),
            state: Mutex::new(State::default()),
        }
    }

    /// The queue group server instances share, in place of the root module's full type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

impl Link for Nats {
    const NAME: &'static str = "nats";

    fn capabilities(&self) -> Capabilities {
        let max_payload = self.max_payload.load(Ordering::Relaxed);
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerPublisher)
            .miss_signal(true)
            .max_frame((max_payload > 0).then_some(max_payload))
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        servers(&self.url)?;
        match &self.group {
            Some(group) => check_group(group)?,
            None => self.default_group = Some(queue_group(&format!("{:#}", app.root().name()))),
        }
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the NATS link's queue group is unset: `listen` ran before `prepare`")?;
        let client = open(&self.url).await?;
        self.max_payload.store(client.server_info().max_payload as u64, Ordering::Relaxed);

        // Every subscription is made before any lane starts, so a failure leaves nothing
        // subscribed: a dropped `Subscriber` unsubscribes.
        let mut lanes = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            match client.queue_subscribe(pattern.as_str().to_owned(), group.clone()).await {
                Ok(subscriber) => lanes.push(subscriber),
                Err(error) => {
                    drop(lanes);
                    let _ = client.drain().await;
                    return Err(format!("the NATS link could not subscribe to `{pattern}`: {error}").into());
                }
            }
        }
        let control = match client.subscribe(CONTROL).await {
            Ok(subscriber) => subscriber,
            Err(error) => {
                drop(lanes);
                let _ = client.drain().await;
                return Err(format!("the NATS link could not subscribe to `{CONTROL}`: {error}").into());
            }
        };
        if let Err(error) = client.flush().await {
            let _ = client.drain().await;
            return Err(format!("the NATS link's subscriptions did not reach the server: {error}").into());
        }

        let (phase, _) = watch::channel(Phase::Serving);
        let side = Arc::new(ServerSide { client, codec: self.codec, calls: Arc::new(Calls::new()), phase });
        let (deliveries, inbound) = mpsc::unbounded_channel();
        for subscriber in lanes {
            tokio::spawn(request_lane(Arc::clone(&side), subscriber, deliveries.clone()));
        }
        tokio::spawn(control_lane(Arc::clone(&side), control, deliveries));
        lock(&self.state).server = Some(side);
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        // async-nats reconnects by itself and resubscribes the inbox, but what was published to
        // the inbox while the connection was down is gone, so a disconnect ends the reply lane:
        // `RpcClient` fails the calls waiting on it `Unavailable` and connects again for the next.
        let (lost, disconnected) = watch::channel(false);
        let options = ConnectOptions::new().event_callback(move |event| {
            if matches!(event, Event::Disconnected) {
                lost.send_replace(true);
            }
            async {}
        });
        let client = open_with(&self.url, options).await?;
        self.max_payload.store(client.server_info().max_payload as u64, Ordering::Relaxed);
        let prefix = client.new_inbox();
        let replies = client.subscribe(format!("{prefix}.*")).await?;
        // The reply subscription reaches the server before the first request can, so no reply
        // arrives ahead of it.
        client.flush().await?;

        let side = Arc::new(ClientSide {
            client: client.clone(),
            codec: self.codec,
            prefix,
            max_payload: Arc::clone(&self.max_payload),
            calls: Mutex::new(HashMap::new()),
        });
        lock(&self.state).client = Some(client);
        let (frames, replies_out) = mpsc::unbounded_channel();
        tokio::spawn(route_replies(Arc::clone(&side), replies, frames, disconnected));

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
        if let Some(side) = server {
            side.phase.send_replace(Phase::Draining);
        }
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
            if let Err(error) = shut(&side.client).await {
                failure = Some(error);
            }
        }
        if let Some(client) = client
            && let Err(error) = shut(&client).await
        {
            failure = Some(error);
        }
        match failure {
            Some(error) => Err(format!("the NATS link did not close cleanly: {error}").into()),
            None => Ok(()),
        }
    }
}

/// Closes one connection once the server has routed everything it published.
///
/// async-nats's `publish` and `drain` only queue a command for the connection's task, and a task
/// that finds its command channel closed exits without writing what it already took from it. Once
/// `close` drops the last `Client`, the replies of the calls the drain let finish could be lost
/// that way. NATS routes one connection's messages in order, so a message to the connection's own
/// inbox coming back shows every earlier one was routed. Skipped while disconnected: async-nats
/// writes nothing it queued until it reconnects, and the close drops the connection first.
async fn shut(client: &Client) -> Result<(), BoxError> {
    if client.connection_state() == Connection::Connected {
        let subject = client.new_inbox();
        let mut echo = client.subscribe(subject.clone()).await?;
        client.publish(subject, Bytes::new()).await?;
        if echo.next().await.is_none() {
            return Err("the connection closed before its last messages were routed".into());
        }
    }
    client.drain().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// The server side of a bound link: the connection, the calls it holds, and the phase its lanes
/// watch.
struct ServerSide {
    client: Client,
    codec: Codec,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
}

impl ServerSide {
    async fn on_request(&self, message: Message, deliveries: &mpsc::UnboundedSender<Delivery>) {
        let pattern = message.subject.to_string();
        let kind = kind_of(message.headers.as_ref());
        let headers = call_headers(message.headers.as_ref());
        let (frame, path) = match (message.reply, kind.as_deref()) {
            (None, None) => {
                let frame = Frame::Evt { pattern, headers, data: Data::new(message.payload) };
                let _ = deliveries.send(Delivery { frame, reply: None, ack: Ack::none() });
                return;
            }
            (Some(reply), None) => {
                let (id, path) = self.hold(reply.to_string());
                (Frame::Req { id, pattern, headers, data: Data::new(message.payload) }, path)
            }
            (Some(reply), Some(OPEN)) => {
                let subject = reply.to_string();
                let (id, path) = self.hold(subject.clone());
                // The caller holds the request's items until this arrives, so they reach the
                // control lane after the call is held here.
                let opened = publish(&self.client, subject, None, &CallHeaders::new(), Some(OPENED), Bytes::new()).await;
                if let Err(error) = opened {
                    tracing::warn!(%error, pattern, "the NATS link could not acknowledge a streamed request");
                }
                (Frame::Open { id, pattern, headers }, path)
            }
            (reply, Some(kind)) => {
                tracing::warn!(pattern, kind, reply = reply.is_some(), "the NATS link dropped a request-lane message of an unknown kind");
                return;
            }
        };
        let _ = deliveries.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
    }

    fn on_control(&self, message: Message, deliveries: &mpsc::UnboundedSender<Delivery>) {
        let Some(key) = message.reply.as_ref().map(ToString::to_string) else { return };
        let frame = match kind_of(message.headers.as_ref()).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(message.payload) }, path)),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            let _ = deliveries.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under its reply subject, the key its control messages carry.
    fn hold(&self, subject: String) -> (u64, ReplyPath) {
        let key = subject.clone();
        let client = self.client.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let client = client.clone();
            let calls = Arc::clone(&calls);
            let subject = subject.clone();
            Box::pin(async move {
                if is_terminal(&frame) {
                    calls.release(&subject);
                }
                let bytes = codec.encode_frame(&frame)?;
                client.publish(subject, bytes).await?;
                Ok(())
            })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

/// The calls a server holds, by the key their control messages carry, each with its local id and
/// its reply path. The local id is this link's own, unique across every caller, so a frame's
/// `id` never collides between two clients that each count from zero.
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

async fn request_lane(side: Arc<ServerSide>, mut subscriber: Subscriber, deliveries: mpsc::UnboundedSender<Delivery>) {
    let mut phase = side.phase.subscribe();
    let mut draining = false;
    loop {
        tokio::select! {
            changed = phase.changed(), if !draining => {
                let now = if changed.is_ok() { *phase.borrow_and_update() } else { Phase::Closed };
                match now {
                    Phase::Serving => {}
                    // NATS's drain: the server stops sending to this subscription, what it already
                    // sent is still delivered, and then the subscriber ends.
                    Phase::Draining => {
                        draining = true;
                        if let Err(error) = subscriber.drain().await {
                            tracing::warn!(%error, "the NATS link could not drain a subscription");
                            break;
                        }
                    }
                    Phase::Closed => break,
                }
            }
            message = subscriber.next() => match message {
                Some(message) => side.on_request(message, &deliveries).await,
                None => break,
            },
        }
    }
}

/// Runs until the link closes, or once draining until no call is held, since a held call may
/// still receive its items or a `cancel`; the inbound stream ends when this and every request
/// lane have.
async fn control_lane(side: Arc<ServerSide>, mut subscriber: Subscriber, deliveries: mpsc::UnboundedSender<Delivery>) {
    let mut phase = side.phase.subscribe();
    let mut count = side.calls.count.subscribe();
    loop {
        let draining = *phase.borrow() == Phase::Draining;
        tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() == Phase::Closed {
                    break;
                }
            }
            () = async { let _ = count.wait_for(|held| *held == 0).await; }, if draining => break,
            message = subscriber.next() => match message {
                Some(message) => side.on_control(message, &deliveries),
                None => break,
            },
        }
    }
}

/// The client side: one inbox subscription for every reply, each call's reply subject
/// `<prefix>.<id>`.
struct ClientSide {
    client: Client,
    codec: Codec,
    prefix: String,
    max_payload: Arc<AtomicU64>,
    calls: Mutex<HashMap<u64, ClientCall>>,
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
    async fn send(&self, pattern: Pattern, frame: Frame) -> Result<(), BoxError> {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                self.fits(data.len())?;
                lock(&self.calls).insert(id, ClientCall::Unary { pattern: pattern.clone() });
                let sent = publish(&self.client, pattern.to_string(), Some(self.reply_subject(id)), &headers, None, data.into_bytes()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                self.fits(data.len())?;
                publish(&self.client, pattern.to_string(), None, &headers, None, data.into_bytes()).await
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { pattern: pattern.clone(), gate: Some(gate), queue });
                tokio::spawn(pump(self.client.clone(), self.reply_subject(id), opened, queued));
                let sent = publish(&self.client, pattern.to_string(), Some(self.reply_subject(id)), &headers, Some(OPEN), Bytes::new()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
                self.fits(data.len())?;
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
                    Some(ClientCall::Unary { .. }) => publish_control(&self.client, self.reply_subject(id), CANCEL, Bytes::new()).await,
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

    fn reply_subject(&self, id: u64) -> String {
        format!("{}.{id}", self.prefix)
    }

    fn id_of(&self, subject: &str) -> Option<u64> {
        subject.strip_prefix(self.prefix.as_str())?.strip_prefix('.')?.parse().ok()
    }

    fn fits(&self, size: usize) -> Result<(), BoxError> {
        let limit = self.max_payload.load(Ordering::Relaxed);
        if limit > 0 && size as u64 > limit {
            return Err(Box::new(FrameTooLarge { size: size as u64, limit }));
        }
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

    fn take(&self, id: u64) -> Option<ClientCall> {
        lock(&self.calls).remove(&id)
    }
}

/// Routes the inbox's messages until the connection is lost or the inbox subscription ends;
/// either ends the reply lane.
async fn route_replies(
    side: Arc<ClientSide>,
    mut replies: Subscriber,
    frames: mpsc::UnboundedSender<Frame>,
    mut disconnected: watch::Receiver<bool>,
) {
    loop {
        let message = tokio::select! {
            _ = disconnected.wait_for(|lost| *lost) => break,
            message = replies.next() => match message {
                Some(message) => message,
                None => break,
            },
        };
        let Some(id) = side.id_of(message.subject.as_str()) else { continue };
        // No-responders arrives on the reply subject after the publish returned, so the miss is
        // reported as the reply rather than as the `send` failure.
        if message.status == Some(StatusCode::NO_RESPONDERS) {
            if let Some(call) = side.take(id)
                && frames.send(no_destination(id, call.pattern())).is_err()
            {
                break;
            }
            continue;
        }
        if kind_of(message.headers.as_ref()).as_deref() == Some(OPENED) {
            side.open_gate(id);
            continue;
        }
        let frame = match side.codec.decode_frame(&message.payload) {
            Ok(frame) => with_id(frame, id),
            Err(error) => {
                tracing::warn!(%error, "the NATS link dropped a reply that does not decode");
                continue;
            }
        };
        if is_terminal(&frame) {
            side.take(id);
        }
        if frames.send(frame).is_err() {
            break;
        }
    }
}

/// Publishes one streamed request's control frames once the server has acknowledged its `open`.
async fn pump(client: Client, reply: String, opened: oneshot::Receiver<()>, mut queued: mpsc::UnboundedReceiver<Frame>) {
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
        if let Err(error) = publish_control(&client, reply.clone(), kind, body).await {
            tracing::debug!(%error, kind, "the NATS link could not publish a control frame");
        }
    }
}

async fn publish(
    client: &Client,
    subject: String,
    reply: Option<String>,
    headers: &CallHeaders,
    kind: Option<&str>,
    body: Bytes,
) -> Result<(), BoxError> {
    let map = nats_headers(headers, kind);
    match (reply, map) {
        (None, None) => client.publish(subject, body).await?,
        (None, Some(map)) => client.publish_with_headers(subject, map, body).await?,
        (Some(reply), None) => client.publish_with_reply(subject, reply, body).await?,
        (Some(reply), Some(map)) => client.publish_with_reply_and_headers(subject, reply, map, body).await?,
    }
    Ok(())
}

/// A control-lane message: the call's reply subject is its key on every server instance.
async fn publish_control(client: &Client, reply: String, kind: &str, body: Bytes) -> Result<(), BoxError> {
    publish(client, CONTROL.to_owned(), Some(reply), &CallHeaders::new(), Some(kind), body).await
}

fn nats_headers(headers: &CallHeaders, kind: Option<&str>) -> Option<HeaderMap> {
    if headers.is_empty() && kind.is_none() {
        return None;
    }
    let mut map = HeaderMap::new();
    for (name, value) in headers.iter() {
        map.append(name, value);
    }
    if let Some(kind) = kind {
        map.insert(KIND, kind);
    }
    Some(map)
}

fn kind_of(headers: Option<&HeaderMap>) -> Option<String> {
    headers?.get(KIND).map(|value| value.as_str().to_owned())
}

fn call_headers(headers: Option<&HeaderMap>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for (name, values) in headers.iter() {
            let name: &str = name.as_ref();
            if name == KIND {
                continue;
            }
            for value in values {
                out.insert(name, value.as_str());
            }
        }
    }
    out
}

/// What a caller receives for a miss this link signals: `Unavailable` with
/// `reason: "no_destination"`, as `RpcClient` maps a `NoDestination` from `send`.
fn no_destination(id: u64, pattern: &Pattern) -> Frame {
    let mut details = Details::new();
    details.push(Detail::ErrorInfo { reason: "no_destination".to_owned(), domain: DOMAIN.to_owned(), metadata: BTreeMap::new() });
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, format!("nothing listens on pattern `{pattern}`"), details) }
}

/// The reply frame with the caller's own `id`, read from the reply subject: the server wrote its
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

fn servers(url: &str) -> Result<Vec<ServerAddr>, BoxError> {
    let servers = url
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(|server| server.parse::<ServerAddr>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("the NATS link's url does not parse: {error}"))?;
    if servers.is_empty() {
        return Err("the NATS link's url names no server".into());
    }
    Ok(servers)
}

async fn open(url: &str) -> Result<Client, BoxError> {
    open_with(url, ConnectOptions::new()).await
}

async fn open_with(url: &str, options: ConnectOptions) -> Result<Client, BoxError> {
    let servers = servers(url)?;
    Ok(options.connect(servers).await?)
}

/// A queue group name from the root module's path: NATS refuses whitespace and the wildcards in
/// one.
fn queue_group(text: &str) -> String {
    text.chars().map(|c| if c.is_whitespace() || c == '*' || c == '>' { '_' } else { c }).collect()
}

fn check_group(group: &str) -> Result<(), BoxError> {
    if group.is_empty() || group.chars().any(|c| c.is_whitespace() || c == '*' || c == '>') {
        return Err(format!(
            "the NATS link's group `{group}` is not a queue group name: it must be non-empty, with no whitespace, `*` or `>`"
        )
        .into());
    }
    Ok(())
}

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
