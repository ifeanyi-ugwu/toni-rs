//! The gateway attribute's limits past what the WebSocket conformance suite pins, each on a gateway
//! of its own on the standalone server: one frame over `message_limit`, `max_inflight` of two, and
//! `max_outbound` under both overflow policies, the slow consumer's Close written before anything
//! queued. The suite (`tests/conformance.rs`) runs a message over the limit after reassembly,
//! `max_connections`, `max_inflight` of one, a streamed answer under `max_outbound`, and
//! keep-alive. The counts are written as integer literals, which the attribute rewrites to
//! `Count::Max`, and once as an expression.
//!
//! The outbound tests run on the current-thread runtime `#[tokio::test]` builds, so a handler's
//! sends all reach the queue before the connection's loop next runs.

mod support;

use serde_json::json;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use ulo::{Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::Count;
use ulo_ws::{Connection, DisconnectReason, Frame, OnDisconnect, Payload, WsCx};

use support::{Record, Running, close_frame, hang_up, next_json, next_message, send_json};

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

macro_rules! record_ends {
    ($($gateway:ty),*) => {$(
        impl OnDisconnect for $gateway {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.ended.0.push((conn.info().path().to_owned(), why));
            }
        }
    )*};
}

record_ends!(Strict, Lossy, Small);

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
        m.controller::<Parallel>();
        m.controller::<Strict>();
        m.controller::<Lossy>();
        m.controller::<Small>();
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
    let app = Running::start(root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    Started { app, ended, gate }
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
