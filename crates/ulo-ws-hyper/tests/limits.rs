//! The gateway attribute's limits past what the WebSocket conformance suite pins, each on a gateway
//! of its own on the standalone server: one frame over `message_limit`, `max_inflight` of two,
//! `max_outbound` under both overflow policies, the slow consumer's Close written before anything
//! queued, and the two in-flight bounds' own values: a connection's 64 by default, and the
//! server's across its connections. The suite (`tests/conformance.rs`) runs a message over the limit after reassembly,
//! `max_connections`, `max_inflight` of one, a streamed answer under `max_outbound`, and
//! keep-alive. The counts are written as integer literals, which the attribute rewrites to
//! `Count::Max`, and once as an expression.
//!
//! The outbound tests run on the current-thread runtime `#[tokio::test]` builds, so a handler's
//! sends all reach the queue before the connection's loop next runs.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{Semaphore, watch};
use tokio_tungstenite::tungstenite::Message;
use ulo::{Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::Count;
use ulo_ws::{Connection, DisconnectReason, Frame, MessagesRead, OnDisconnect, Payload, WsCx};

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

/// The `pass` handlers: each counted as it starts and while it runs, then let through one at a
/// time by the test.
#[derive(Clone)]
struct Turnstile {
    let_through: Arc<Semaphore>,
    started: Arc<AtomicUsize>,
    running: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
}

impl Default for Turnstile {
    fn default() -> Self {
        Turnstile {
            let_through: Arc::new(Semaphore::new(0)),
            started: Arc::default(),
            running: Arc::default(),
            most: Arc::default(),
        }
    }
}

impl Turnstile {
    async fn pass(&self) -> &'static str {
        self.started.fetch_add(1, Ordering::AcqRel);
        let running = self.running.fetch_add(1, Ordering::AcqRel) + 1;
        self.most.fetch_max(running, Ordering::AcqRel);
        self.let_through.acquire().await.expect("the turnstile is never closed").forget();
        self.running.fetch_sub(1, Ordering::AcqRel);
        "passed"
    }

    /// Waits until `n` handlers have started.
    async fn started(&self, n: usize) {
        within(&format!("{n} handlers starting"), async {
            while self.started.load(Ordering::Acquire) < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }

    fn release(&self, n: usize) {
        self.let_through.add_permits(n);
    }

    fn most(&self) -> usize {
        self.most.load(Ordering::Acquire)
    }
}

/// Every message a turnstile's handler, the gateway's `max_inflight` left at its default.
#[injectable]
struct Queue {
    turnstile: Dep<Turnstile>,
}

#[routes]
#[ulo_ws::gateway(path = "/queue", port = own)]
impl Queue {
    #[ulo_ws::message("pass")]
    async fn pass(&self) -> &'static str {
        self.turnstile.pass().await
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
    turnstile: Turnstile,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.ended.clone());
        m.value(self.gate.clone());
        m.value(self.turnstile.clone());
        m.controller::<Queue>();
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
    turnstile: Turnstile,
}

async fn start() -> Started {
    start_on(ulo_ws_hyper::Server::new("127.0.0.1:0")).await
}

async fn start_on(server: ulo_ws_hyper::Server) -> Started {
    let ended = Ended(Record::new());
    let gate = Gate { opened: watch::channel(false).0, log: Record::new() };
    let turnstile = Turnstile::default();
    let root = Root { ended: ended.clone(), gate: gate.clone(), turnstile: turnstile.clone() };
    let app = Running::start(root, server).await;
    Started { app, ended, gate, turnstile }
}

/// Reads `n` answers to `pass` on `socket`.
async fn passed(socket: &mut Socket, n: usize) {
    for _ in 0..n {
        let answer = next_json(socket).await;
        assert_eq!(answer["data"], "passed", "{answer}");
    }
}

/// One more message than the connection's default bound: 64 handlers run at once, and the 65th
/// starts only once one of them has answered.
#[tokio::test]
async fn a_connection_holds_64_messages_in_flight_by_default() {
    let Started { app, turnstile, .. } = start().await;
    let mut socket = app.connect("/queue", &[]).await;
    for id in 1..=66 {
        send_json(&mut socket, &json!({ "event": "pass", "id": id })).await;
    }
    turnstile.started(64).await;
    turnstile.release(1);
    passed(&mut socket, 1).await;
    turnstile.started(65).await;
    turnstile.release(65);
    passed(&mut socket, 65).await;
    assert_eq!(turnstile.most(), 64, "the most messages one connection had in flight at once");
    hang_up(socket).await;
    app.stop().await;
}

/// One message on each of five connections, the server bounded at three across them: three run at
/// once, and each answer lets one more start, the connections having stopped reading meanwhile.
#[tokio::test]
async fn the_server_s_connections_together_stop_at_its_bound_and_resume_as_places_free() {
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0").server_max_inflight(Count::Max(3));
    let Started { app, turnstile, .. } = start_on(server).await;
    let mut sockets = Vec::new();
    for id in 1..=5 {
        let mut socket = app.connect("/queue", &[]).await;
        send_json(&mut socket, &json!({ "event": "pass", "id": id })).await;
        sockets.push(socket);
    }
    turnstile.started(3).await;
    for more in 4..=5 {
        turnstile.release(1);
        turnstile.started(more).await;
    }
    turnstile.release(3);
    for socket in &mut sockets {
        passed(socket, 1).await;
    }
    assert_eq!(turnstile.most(), 3, "the most messages the server's connections had in flight at once");
    for socket in sockets {
        hang_up(socket).await;
    }
    app.stop().await;
}

/// Waits until `count` reads at least `n`.
async fn read_at_least(count: &MessagesRead, n: usize) {
    within(&format!("{n} messages read"), async {
        while count.get() < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

/// The connections stop reading, not only handling, once the server's places are taken: the
/// messages read off the sockets rise by at most one per connection open when the bound was
/// reached, each waiting unhandled for a place, and a connection opened after reads nothing. The
/// rest stays in the clients' TCP buffers until places free, and then every message is answered.
#[tokio::test]
async fn a_saturated_server_s_connections_stop_reading_off_their_sockets() {
    const BOUND: usize = 2;
    const MORE: usize = 3;
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0").server_max_inflight(Count::Max(BOUND as u32));
    let read = server.messages_read();
    let Started { app, turnstile, .. } = start_on(server).await;
    let mut sockets = Vec::new();
    for _ in 0..3 {
        sockets.push(app.connect("/queue", &[]).await);
    }
    for id in 0..BOUND {
        send_json(&mut sockets[0], &json!({ "event": "pass", "id": id })).await;
    }
    turnstile.started(BOUND).await;
    for socket in &mut sockets {
        for id in 0..MORE {
            send_json(socket, &json!({ "event": "pass", "id": 100 + id })).await;
        }
    }
    // The connections idle in a read when the bound was reached each read one message, which
    // waits for a place; the one that reached it reads nothing more.
    read_at_least(&read, BOUND + sockets.len() - 1).await;
    tokio::time::sleep(SETTLE).await;
    let saturated = read.get();
    assert!(
        saturated <= BOUND + sockets.len(),
        "{saturated} messages read off the sockets of a server saturated at {BOUND} places, over {} connections",
        sockets.len()
    );
    let mut late = app.connect("/queue", &[]).await;
    send_json(&mut late, &json!({ "event": "pass", "id": 200 })).await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(read.get(), saturated, "a connection opened while every place was taken read a message");
    sockets.push(late);

    let total = BOUND + 3 * MORE + 1;
    turnstile.release(total);
    passed(&mut sockets[0], BOUND + MORE).await;
    for socket in &mut sockets[1..3] {
        passed(socket, MORE).await;
    }
    passed(&mut sockets[3], 1).await;
    assert_eq!(read.get(), total, "every message read once places freed");
    assert_eq!(turnstile.most(), BOUND, "the most messages the server's connections had in flight at once");
    for socket in sockets {
        hang_up(socket).await;
    }
    app.stop().await;
}

/// How long the test waits for a read it expects not to happen.
const SETTLE: Duration = Duration::from_millis(200);

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
