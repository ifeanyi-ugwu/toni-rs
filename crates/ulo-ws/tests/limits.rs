//! The gateway attribute's limits, each on a gateway of its own on the standalone server:
//! `message_limit` after reassembly, `max_connections`, `max_inflight`, `max_outbound` under both
//! overflow policies, and keep-alive, where a Pong that misses `pong_timeout` ends the connection.
//! The counts are written as integer literals, which the attribute rewrites to `Count::Max`, and
//! once as an expression.
//!
//! The outbound tests run on the current-thread runtime `#[tokio::test]` builds, so a handler's
//! sends all reach the queue before the connection's loop next runs.

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::Frame as WireFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use ulo::{Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::Count;
use ulo_ws::{Connection, DisconnectReason, Frame, OnDisconnect, Payload, WsCx};

use support::{Record, Running, Socket, close_frame, hang_up, next_json, next_message, send_json, within};

/// Why each connection on a gateway ended, as (gateway path, reason).
#[derive(Clone)]
struct Ended(Record<(String, DisconnectReason)>);

/// A gate the `hold` handler waits behind, opened by the `open` handler or by the test, and the
/// order things happened in.
#[derive(Clone)]
struct Gate {
    opened: watch::Sender<bool>,
    log: Record<&'static str>,
}

impl Gate {
    fn open(&self, by: &'static str) {
        self.log.push(by);
        self.opened.send_replace(true);
    }

    /// Waits until the gate is open.
    async fn hold(&self) -> &'static str {
        self.log.push("hold started");
        let mut opened = self.opened.subscribe();
        let _ = opened.wait_for(|opened| *opened).await;
        self.log.push("hold done");
        "held"
    }
}

/// `Connection::send` `n` times, the frames `"1"` to `"n"` as they stand.
async fn burst(conn: &Connection, n: u32) {
    for i in 1..=n {
        let _ = conn.send(Frame::text(i.to_string())).await;
    }
}

#[injectable]
struct Serial {
    gate: Dep<Gate>,
}

#[routes]
#[ulo_ws::gateway(path = "/serial", port = own, max_inflight = 1)]
impl Serial {
    #[ulo_ws::message("hold")]
    async fn hold(&self) -> &'static str {
        self.gate.hold().await
    }

    #[ulo_ws::message("open")]
    fn open(&self) -> &'static str {
        self.gate.open("opened by a message");
        "opened"
    }

    #[ulo_ws::message("noop")]
    fn noop(&self) {}
}

#[injectable]
struct Parallel {
    gate: Dep<Gate>,
}

#[routes]
#[ulo_ws::gateway(path = "/parallel", port = own, max_inflight = Count::Max(2))]
impl Parallel {
    #[ulo_ws::message("hold")]
    async fn hold(&self) -> &'static str {
        self.gate.hold().await
    }

    #[ulo_ws::message("open")]
    fn open(&self) -> &'static str {
        self.gate.open("opened by a message");
        "opened"
    }

    #[ulo_ws::message("noop")]
    fn noop(&self) {}
}

#[injectable]
struct Strict {
    ended: Dep<Ended>,
}

#[routes]
#[ulo_ws::gateway(path = "/strict", port = own, max_outbound = 1)]
impl Strict {
    #[ulo_ws::message("burst")]
    async fn burst(&self, n: Payload<u32>, cx: WsCx) {
        burst(cx.conn(), n.0).await;
    }

    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

#[injectable]
struct Lossy {
    ended: Dep<Ended>,
}

#[routes]
#[ulo_ws::gateway(path = "/lossy", port = own, max_outbound = 2, overflow = drop_oldest)]
impl Lossy {
    #[ulo_ws::message("burst")]
    async fn burst(&self, n: Payload<u32>, cx: WsCx) {
        burst(cx.conn(), n.0).await;
    }

    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

#[injectable]
struct Small {
    ended: Dep<Ended>,
}

#[routes]
#[ulo_ws::gateway(path = "/small", port = own, message_limit = 128, max_connections = 1)]
impl Small {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

#[injectable]
struct Heartbeat {
    ended: Dep<Ended>,
}

#[routes]
#[ulo_ws::gateway(
    path = "/heartbeat",
    port = own,
    ping_interval = ulo::Bound::After(Duration::from_millis(100)),
    pong_timeout = ulo::Bound::After(Duration::from_millis(150)),
)]
impl Heartbeat {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

macro_rules! record_ends {
    ($($gateway:ty),*) => {$(
        impl OnDisconnect for $gateway {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.ended.0.push((conn.info().path().to_owned(), why));
            }
        }
    )*};
}

record_ends!(Strict, Lossy, Small, Heartbeat);

struct Root {
    ended: Ended,
    gate: Gate,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.ended.clone());
        m.value(self.gate.clone());
        m.controller::<Serial>();
        m.controller::<Parallel>();
        m.controller::<Strict>();
        m.controller::<Lossy>();
        m.controller::<Small>();
        m.controller::<Heartbeat>();
    }
}

struct Started {
    app: Running,
    ended: Ended,
    gate: Gate,
}

async fn start() -> Started {
    let ended = Ended(Record::new());
    let gate = Gate { opened: watch::channel(false).0, log: Record::new() };
    let root = Root { ended: ended.clone(), gate: gate.clone() };
    let app = Running::start(root, ulo_ws::Server::new("127.0.0.1:0")).await;
    Started { app, ended, gate }
}

async fn exchange(socket: &mut Socket, event: &str, id: u32) -> serde_json::Value {
    send_json(socket, &json!({ "event": event, "id": id })).await;
    next_json(socket).await
}

#[tokio::test]
async fn a_message_over_the_limit_closes_with_1009() {
    let Started { app, ended, .. } = start().await;
    let mut socket = app.connect("/small", &[]).await;
    send_json(&mut socket, &json!({ "event": "echo", "id": 1, "data": "short" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "short" }));

    send_json(&mut socket, &json!({ "event": "echo", "id": 2, "data": "x".repeat(200) })).await;
    assert_eq!(close_frame(&mut socket).await.map(|(code, _)| code), Some(1009));
    hang_up(socket).await;
    let ends = ended.0.at_least(1, "the oversized connection's end").await;
    assert_eq!(ends, vec![("/small".to_owned(), DisconnectReason::ProtocolError)]);
    app.stop().await;
}

#[tokio::test]
async fn the_message_limit_counts_a_message_after_reassembly() {
    let Started { app, .. } = start().await;
    let mut socket = app.connect("/small", &[]).await;
    // Two fragments, the first 80 bytes and the second the rest: each under the 128-byte limit,
    // together over it.
    let message = json!({ "event": "echo", "id": 1, "data": "y".repeat(140) }).to_string();
    let (first, rest) = message.split_at(80);
    let fragments = [
        WireFrame::message(first.as_bytes().to_vec(), OpCode::Data(Data::Text), false),
        WireFrame::message(rest.as_bytes().to_vec(), OpCode::Data(Data::Continue), true),
    ];
    for fragment in fragments {
        socket.send(Message::Frame(fragment)).await.unwrap_or_else(|error| panic!("sending a fragment failed: {error}"));
    }
    assert_eq!(close_frame(&mut socket).await.map(|(code, _)| code), Some(1009));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn an_integer_max_connections_admits_that_many_and_closes_the_next_with_1013() {
    let Started { app, .. } = start().await;
    let mut first = app.connect("/small", &[]).await;
    let mut second = app.connect("/small", &[]).await;
    assert_eq!(close_frame(&mut second).await, Some((1013, "too many connections".to_owned())));
    hang_up(second).await;
    send_json(&mut first, &json!({ "event": "echo", "id": 1, "data": "still here" })).await;
    assert_eq!(next_json(&mut first).await, json!({ "id": 1, "data": "still here" }));
    hang_up(first).await;
    app.stop().await;
}

#[tokio::test]
async fn max_inflight_of_two_runs_a_second_message_beside_the_first() {
    let Started { app, gate, .. } = start().await;
    let mut socket = app.connect("/parallel", &[]).await;
    // `hold` finishes only once `open` has run, so both answers prove the two ran side by side.
    send_json(&mut socket, &json!({ "event": "hold", "id": 1 })).await;
    send_json(&mut socket, &json!({ "event": "open", "id": 2 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": "opened" }));
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "held" }));
    assert_eq!(gate.log.snapshot(), vec!["hold started", "opened by a message", "hold done"]);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn max_inflight_of_one_reads_nothing_more_until_the_message_in_flight_is_answered() {
    let Started { app, gate, .. } = start().await;
    let mut socket = app.connect("/serial", &[]).await;
    send_json(&mut socket, &json!({ "event": "hold", "id": 1 })).await;
    gate.log.at_least(1, "`hold` starting").await;
    send_json(&mut socket, &json!({ "event": "open", "id": 2 })).await;
    // A round trip on a second connection, which has a message in flight of its own to spend:
    // had the first connection kept reading, `open` would have run by the time it completes.
    let mut other = app.connect("/serial", &[]).await;
    assert_eq!(exchange(&mut other, "noop", 9).await, json!({ "id": 9, "complete": true }));
    gate.open("opened by the test");

    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "held" }));
    assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": "opened" }));
    assert_eq!(gate.log.snapshot(), vec!["hold started", "opened by the test", "hold done", "opened by a message"]);
    hang_up(socket).await;
    hang_up(other).await;
    app.stop().await;
}

#[tokio::test]
async fn an_outbound_queue_over_its_limit_closes_with_slow_consumer() {
    let Started { app, ended, .. } = start().await;
    let mut socket = app.connect("/strict", &[]).await;
    send_json(&mut socket, &json!({ "event": "burst", "id": 1, "data": 3 })).await;
    // The overflow discards what was queued, so the Close frame is the first thing written.
    assert_eq!(close_frame(&mut socket).await, Some((1008, "slow consumer".to_owned())));
    hang_up(socket).await;
    let ends = ended.0.at_least(1, "the slow consumer's end").await;
    assert_eq!(ends, vec![("/strict".to_owned(), DisconnectReason::ServerClose { code: 1008 })]);
    app.stop().await;
}

#[tokio::test]
async fn drop_oldest_keeps_the_newest_messages_and_the_connection() {
    let Started { app, .. } = start().await;
    let mut socket = app.connect("/lossy", &[]).await;
    // Fire-and-forget, so the two places hold the burst alone: "1" to "3" are each dropped by a
    // later send.
    send_json(&mut socket, &json!({ "event": "burst", "data": 5 })).await;
    assert_eq!(next_message(&mut socket).await, Message::text("4"));
    assert_eq!(next_message(&mut socket).await, Message::text("5"));
    send_json(&mut socket, &json!({ "event": "echo", "id": 2, "data": "after" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": "after" }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn a_pong_that_misses_pong_timeout_ends_the_connection_as_lost() {
    let Started { app, ended, .. } = start().await;
    let mut socket = app.connect("/heartbeat", &[]).await;
    let ping = within("the first keep-alive Ping", socket.next()).await;
    assert!(matches!(ping, Some(Ok(Message::Ping(_)))), "expected a Ping, got {ping:?}");
    // The client answers a Ping on its next poll. It does not poll again until the server has
    // given up on the Pong.
    let ends = ended.0.at_least(1, "the end of a connection whose Pong never came").await;
    assert_eq!(ends, vec![("/heartbeat".to_owned(), DisconnectReason::Lost)]);
    // The Pong goes out late, and the server ends the connection without a Close frame.
    let ending = within("the connection's end", async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Ping(_))) => continue,
                other => return other,
            }
        }
    })
    .await;
    assert!(!matches!(ending, Some(Ok(Message::Close(_)))), "a lost connection is dropped, not closed: {ending:?}");
    app.stop().await;
}

#[tokio::test]
async fn a_client_answering_each_ping_outlives_the_pong_timeout() {
    let Started { app, ended, .. } = start().await;
    let mut socket = app.connect("/heartbeat", &[]).await;
    // Three Pings, 100 ms apart, each answered as the next poll begins: longer than the 150 ms a
    // Pong may take.
    let mut pings = 0;
    while pings < 3 {
        match within("a keep-alive Ping", socket.next()).await {
            Some(Ok(Message::Ping(_))) => pings += 1,
            other => panic!("expected a Ping, got {other:?}"),
        }
    }
    send_json(&mut socket, &json!({ "event": "echo", "id": 1, "data": "alive" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "alive" }));
    assert_eq!(ended.0.snapshot(), Vec::new(), "the connection was ended while its client answered every Ping");
    hang_up(socket).await;
    app.stop().await;
}
