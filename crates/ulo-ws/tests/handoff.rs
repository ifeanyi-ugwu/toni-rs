//! Gateways on the HTTP server's own port, reached through the upgrade hand-off `WsModule`
//! registers and served by `ulo_http::Server` over hyper: the hand-off beside an HTTP route,
//! `WsModule`'s defaults, `AfterInit` run from the hand-off's `bound`, the drain's 1001, rooms
//! and broadcast across connections, and a broadcast adapter refusing in its `prepare` failing the
//! HTTP server's `listen()`.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use ulo::{App, BoxError, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Signal, StartupError, injectable, routes};
use ulo_transport::Count;
use ulo_ws::{
    AfterInit, BroadcastAdapter, ConnId, ConnectCx, ConnectRefused, GatewayRef, NodeId, OnConnect, Payload, Rooms, Target, WsCx, WsModule,
};

use support::{Record, Running, close_frame, hang_up, next_json, send_json, upgrade, within};

/// The upgrade header naming the visitor, which `OnConnect` files in the directory.
const NAME: &str = "x-name";

/// Who is connected to the lobby, by the name each upgrade carried.
#[derive(Clone, Default)]
struct Directory(Arc<Mutex<HashMap<String, ConnId>>>);

impl Directory {
    fn insert(&self, name: String, id: ConnId) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(name, id);
    }

    fn get(&self, name: &str) -> Option<ConnId> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(name).copied()
    }
}

/// Each gateway's `AfterInit` call, as (path, namespace).
#[derive(Clone)]
struct Inits(Record<(String, Option<String>)>);

#[derive(Deserialize)]
struct Whisper {
    to: String,
    text: String,
}

#[injectable]
struct Lobby {
    directory: Dep<Directory>,
    inits: Dep<Inits>,
}

#[routes]
#[ulo_ws::gateway(path = "/lobby", namespace = "lobby")]
impl Lobby {
    /// To everyone in the room but the sender.
    #[ulo_ws::message("say")]
    async fn say(&self, text: Payload<String>, cx: WsCx, rooms: Dep<Rooms>) -> Result<(), ulo_ws::BroadcastError> {
        rooms.gateway::<Lobby>().to_room("lobby").except([cx.conn().id()]).emit("said", &text.0).await
    }

    /// To everyone in the room, the sender included, addressed by the gateway's namespace.
    #[ulo_ws::message("mark")]
    async fn mark(&self, text: Payload<String>, rooms: Dep<Rooms>) -> Result<(), ulo_ws::BroadcastError> {
        rooms.namespace("lobby").to_room("lobby").emit("mark", &text.0).await
    }

    /// To the room's members on every gateway: membership alone decides who receives it.
    #[ulo_ws::message("room")]
    async fn room(&self, text: Payload<String>, rooms: Dep<Rooms>) -> Result<(), ulo_ws::BroadcastError> {
        rooms.to_room("lobby").emit("room", &text.0).await
    }

    /// To one connection, found by the name its upgrade carried.
    #[ulo_ws::message("whisper")]
    async fn whisper(&self, whisper: Payload<Whisper>, rooms: Dep<Rooms>) -> Result<bool, ulo_ws::BroadcastError> {
        let Some(id) = self.directory.get(&whisper.0.to) else { return Ok(false) };
        rooms.to_client(id).emit("whispered", &whisper.0.text).await?;
        Ok(true)
    }

    /// To every connection on every gateway.
    #[ulo_ws::message("announce")]
    async fn announce(&self, text: Payload<String>, rooms: Dep<Rooms>) -> Result<(), ulo_ws::BroadcastError> {
        rooms.to_all().emit("announced", &text.0).await
    }

    /// Answered once the connection's connect phase, `OnConnect` included, has finished: no
    /// message is read before it.
    #[ulo_ws::message("ready")]
    fn ready(&self) {}

    #[ulo_ws::message("leave")]
    async fn leave(&self, cx: WsCx) -> Vec<String> {
        cx.conn().leave("lobby").await;
        cx.conn().rooms()
    }
}

/// Every connection joins the room `lobby` and is filed under its name.
impl OnConnect for Lobby {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join("lobby").await;
        if let Some(name) = cx.head().headers().get(NAME).and_then(|name| name.to_str().ok()) {
            self.directory.insert(name.to_owned(), cx.conn().id());
        }
        Ok(())
    }
}

impl AfterInit for Lobby {
    async fn after_init(&self, gw: GatewayRef) {
        self.inits.0.push((gw.path().to_owned(), gw.namespace().map(str::to_owned)));
    }
}

/// A second gateway on the same port, in no room: `to_all` reaches it, a room broadcast does not.
#[injectable]
struct Annex;

#[routes]
#[ulo_ws::gateway(path = "/annex")]
impl Annex {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// A gateway declared for the standalone server, which this app does not bind: the HTTP port does
/// not serve it.
#[injectable]
struct Elsewhere;

#[routes]
#[ulo_ws::gateway(path = "/elsewhere", port = own)]
impl Elsewhere {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

#[injectable]
struct Status;

#[routes]
impl Status {
    #[ulo_http::get("/status")]
    async fn status(&self) -> &'static str {
        "up"
    }
}

struct Root {
    inits: Inits,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().max_connections(Count::Max(2)));
        m.value(Directory::default());
        m.value(self.inits.clone());
        m.controller::<Lobby>();
        m.controller::<Annex>();
        m.controller::<Elsewhere>();
        m.controller::<Status>();
    }
}

async fn start() -> (Running, Inits) {
    let inits = Inits(Record::new());
    let app = Running::start(Root { inits: inits.clone() }, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    (app, inits)
}

/// A lobby connection whose `OnConnect` has run: it is in the room and in the directory.
async fn joined(app: &Running, name: &str) -> support::Socket {
    let mut socket = app.connect("/lobby", &[(NAME, name)]).await;
    send_json(&mut socket, &json!({ "event": "ready", "id": 0 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 0, "complete": true }));
    socket
}

fn event(name: &str, data: Value) -> Value {
    json!({ "event": name, "data": data })
}

/// One HTTP/1.1 GET on its own connection: the status and the body.
async fn get(app: &Running, path: &str) -> (u16, String) {
    within(&format!("GET {path}"), async {
        let mut stream = TcpStream::connect(app.addr).await.unwrap_or_else(|error| panic!("connecting failed: {error}"));
        let request = format!("GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", app.addr);
        stream.write_all(request.as_bytes()).await.unwrap_or_else(|error| panic!("writing GET {path} failed: {error}"));
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap_or_else(|error| panic!("reading GET {path} failed: {error}"));
        let status = response.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_else(|| panic!("bad response {response:?}"));
        let body = response.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default();
        (status, body)
    })
    .await
}

#[tokio::test]
async fn the_http_port_serves_a_gateway_and_a_route_side_by_side() {
    let (app, _) = start().await;
    let mut socket = app.connect("/annex", &[]).await;
    send_json(&mut socket, &json!({ "event": "ping", "id": "a" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": "a", "data": "pong" }));
    assert_eq!(get(&app, "/status").await, (200, "up".to_owned()));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn a_gateway_declared_for_its_own_port_is_not_served_on_the_http_port() {
    let (app, _) = start().await;
    let answer = upgrade(app.addr, "/elsewhere", &[]).await;
    assert_eq!(answer.status, 404, "an upgrade to a `port = own` gateway's path on the HTTP port: {}", answer.body);
    app.stop().await;
}

#[tokio::test]
async fn after_init_runs_once_per_gateway_from_the_hand_off() {
    let (app, inits) = start().await;
    let calls = inits.0.at_least(1, "`AfterInit` on the lobby").await;
    assert_eq!(calls, vec![("/lobby".to_owned(), Some("lobby".to_owned()))]);
    app.stop().await;
}

#[tokio::test]
async fn ws_module_limits_govern_the_gateways_on_the_http_port() {
    let (app, _) = start().await;
    let first = app.connect("/lobby", &[]).await;
    let second = app.connect("/lobby", &[]).await;
    let mut third = app.connect("/lobby", &[]).await;
    assert_eq!(close_frame(&mut third).await, Some((1013, "too many connections".to_owned())));
    hang_up(third).await;
    // The limit is per gateway: the annex still admits.
    let mut annex = app.connect("/annex", &[]).await;
    send_json(&mut annex, &json!({ "event": "ping", "id": 1 })).await;
    assert_eq!(next_json(&mut annex).await, json!({ "id": 1, "data": "pong" }));
    for socket in [first, second, annex] {
        hang_up(socket).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn the_drain_closes_a_same_port_connection_with_going_away() {
    let (app, _) = start().await;
    let mut socket = app.connect("/annex", &[]).await;
    let stopping = tokio::spawn(app.stop());
    assert_eq!(close_frame(&mut socket).await, Some((1001, "server shutting down".to_owned())));
    hang_up(socket).await;
    within("the app's close after its last connection answered", stopping).await.unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn a_room_broadcast_reaches_the_other_members_and_not_the_sender() {
    let (app, _) = start().await;
    let mut ada = joined(&app, "ada").await;
    let mut bob = joined(&app, "bob").await;

    send_json(&mut ada, &json!({ "event": "say", "id": 1, "data": "hello" })).await;
    assert_eq!(next_json(&mut ada).await, json!({ "id": 1, "complete": true }));
    assert_eq!(next_json(&mut bob).await, event("said", json!("hello")));

    // A broadcast to the whole room after it: delivery from one sender is ordered, so had `say`
    // reached ada, its frame would come before this one.
    send_json(&mut ada, &json!({ "event": "mark", "id": 2, "data": "after" })).await;
    assert_eq!(next_json(&mut ada).await, json!({ "id": 2, "complete": true }));
    assert_eq!(next_json(&mut ada).await, event("mark", json!("after")), "the sender received its own `except` broadcast");
    assert_eq!(next_json(&mut bob).await, event("mark", json!("after")));

    hang_up(ada).await;
    hang_up(bob).await;
    app.stop().await;
}

#[tokio::test]
async fn a_client_broadcast_reaches_only_the_connection_it_names() {
    let (app, _) = start().await;
    let mut ada = joined(&app, "ada").await;
    let mut bob = joined(&app, "bob").await;

    send_json(&mut ada, &json!({ "event": "whisper", "id": 1, "data": { "to": "bob", "text": "psst" } })).await;
    assert_eq!(next_json(&mut ada).await, json!({ "id": 1, "data": true }));
    assert_eq!(next_json(&mut bob).await, event("whispered", json!("psst")));

    send_json(&mut bob, &json!({ "event": "mark", "id": 2, "data": "after" })).await;
    assert_eq!(next_json(&mut ada).await, event("mark", json!("after")), "a whisper to bob reached ada");
    assert_eq!(next_json(&mut bob).await, json!({ "id": 2, "complete": true }));
    assert_eq!(next_json(&mut bob).await, event("mark", json!("after")));

    hang_up(ada).await;
    hang_up(bob).await;
    app.stop().await;
}

#[tokio::test]
async fn to_all_reaches_every_gateway_and_a_room_broadcast_only_its_members() {
    let (app, _) = start().await;
    let mut ada = joined(&app, "ada").await;
    let mut annex = app.connect("/annex", &[]).await;

    send_json(&mut ada, &json!({ "event": "room", "id": 1, "data": "members only" })).await;
    send_json(&mut ada, &json!({ "event": "announce", "id": 2, "data": "everyone" })).await;
    // The room broadcast addresses every gateway, the annex's included, and the annex's connection
    // joined no room, so its first broadcast is the announcement.
    assert_eq!(next_json(&mut annex).await, event("announced", json!("everyone")));

    let mut seen = Vec::new();
    while seen.len() < 4 {
        seen.push(next_json(&mut ada).await);
    }
    for expected in [
        json!({ "id": 1, "complete": true }),
        json!({ "id": 2, "complete": true }),
        event("room", json!("members only")),
        event("announced", json!("everyone")),
    ] {
        assert!(seen.contains(&expected), "ada did not receive {expected}; got {seen:?}");
    }

    hang_up(ada).await;
    hang_up(annex).await;
    app.stop().await;
}

#[tokio::test]
async fn a_connection_that_left_the_room_receives_none_of_its_broadcasts() {
    let (app, _) = start().await;
    let mut ada = joined(&app, "ada").await;
    let mut bob = joined(&app, "bob").await;

    send_json(&mut bob, &json!({ "event": "leave", "id": 1 })).await;
    assert_eq!(next_json(&mut bob).await, json!({ "id": 1, "data": [] }));

    send_json(&mut ada, &json!({ "event": "mark", "id": 2, "data": "room" })).await;
    assert_eq!(next_json(&mut ada).await, json!({ "id": 2, "complete": true }));
    assert_eq!(next_json(&mut ada).await, event("mark", json!("room")));
    // Then one bob is sure to receive: the room broadcast, had it reached him, would come first.
    send_json(&mut ada, &json!({ "event": "whisper", "id": 3, "data": { "to": "bob", "text": "still here?" } })).await;
    assert_eq!(next_json(&mut bob).await, event("whispered", json!("still here?")), "bob received a broadcast to a room he left");

    hang_up(ada).await;
    hang_up(bob).await;
    app.stop().await;
}

/// What [`Refusing`]'s `prepare` answers.
const REFUSAL: &str = "the refusing adapter cannot carry broadcasts";

/// A broadcast adapter whose `prepare` refuses, as one tied to a tokio runtime refuses when it has
/// none.
struct Refusing;

impl BroadcastAdapter for Refusing {
    fn prepare(&self) -> Result<(), BoxError> {
        Err(REFUSAL.into())
    }

    fn publish(&self, _target: Target, _frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Err(BoxError::from(REFUSAL)) })
    }

    fn subscribe(&self, _node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        Box::pin(futures_util::stream::empty())
    }
}

/// `WsModule` over [`Refusing`], with one gateway on the HTTP port.
struct RefusingRoot;

impl Module for RefusingRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Refusing));
        m.controller::<Annex>();
    }
}

#[tokio::test]
async fn a_broadcast_adapter_refusing_in_prepare_fails_the_http_server_s_listen() {
    let bound = App::builder(RefusingRoot)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await;
    match bound {
        Err(StartupError::Configure(refused)) => {
            assert!(refused.to_string().contains(REFUSAL), "the refusal does not carry the adapter's: {refused}");
        }
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("handoff")).await;
            panic!("an HTTP server listened with a broadcast adapter that refused in `prepare`");
        }
    }
}
