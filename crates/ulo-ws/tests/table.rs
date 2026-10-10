//! `ulo-ws`'s serving surface without hyper or a socket: a test server builds a `GatewayTable` in
//! `prepare`, answers request heads with its handshake decision, and drives each accepted
//! connection through `Switch::serve` on one end of an in-memory pipe, a WebSocket client
//! speaking on the other. This is the whole of what a server on another HTTP stack or runtime
//! writes around the table.
//!
//! A server whose write of the 101 fails is the host here too: the connection hooks stay paired,
//! `on_disconnect` running once with `Lost` for a connection whose `OnConnect` ran in the
//! handshake, and nothing running for one whose connection phase had not begun. A connection over
//! `max_connections` runs no hook in either refusal mode, and a connect guard decides before the
//! slot is taken.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::header::{ALLOW, CONTENT_TYPE, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_VERSION, UPGRADE};
use http::request::Parts;
use http::{Method, StatusCode, Version};
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, BoxError, DrainToken, Guard, Module, ModuleDef, ModuleIdentity, Mounted, Signal, injectable, routes};
use ulo_http::Upgraded;
use ulo_transport::prepare::Failures;
use ulo_ws::{
    ConnId, ConnectCx, ConnectRefused, Connection, DisconnectReason, GatewayDefaults, GatewayTable, Handshake, OnConnect, OnDisconnect, Ws,
    WsConnect,
};

const WAIT: Duration = Duration::from_secs(5);

/// RFC 6455 §1.3's sample key and the accept value it derives.
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

#[injectable]
struct Plain;

#[routes]
#[ulo_ws::gateway(path = "/plain", port = own)]
impl Plain {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

/// The connections `Held`'s connection phase admitted.
#[derive(Clone, Default)]
struct Joined(Arc<Mutex<Vec<Connection>>>);

#[injectable]
struct Held {
    joined: ulo::Dep<Joined>,
}

#[routes]
#[ulo_ws::gateway(path = "/held", port = own, refuse = handshake)]
impl Held {
    #[ulo_ws::message("noop")]
    fn noop(&self) {}
}

impl OnConnect for Held {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join("lobby").await;
        self.joined.0.lock().unwrap_or_else(PoisonError::into_inner).push(cx.conn().clone());
        Ok(())
    }
}

/// Each connection hook that ran, in order, on `Paired` and `Loose`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hook {
    Connected(ConnId),
    Disconnected(ConnId, DisconnectReason),
}

#[derive(Clone, Default)]
struct Hooks(Arc<Mutex<Vec<Hook>>>);

impl Hooks {
    fn push(&self, hook: Hook) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(hook);
    }

    fn all(&self) -> Vec<Hook> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The connection the last `OnConnect` admitted.
    fn last_connected(&self) -> ConnId {
        self.all()
            .iter()
            .rev()
            .find_map(|hook| match hook {
                Hook::Connected(id) => Some(*id),
                Hook::Disconnected(..) => None,
            })
            .expect("`OnConnect` ran")
    }
}

/// Runs its connection phase in the handshake, one connection at a time.
#[injectable]
struct Paired {
    hooks: ulo::Dep<Hooks>,
}

#[routes]
#[ulo_ws::gateway(path = "/paired", port = own, refuse = handshake, max_connections = 1)]
impl Paired {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

impl OnConnect for Paired {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        self.hooks.push(Hook::Connected(cx.conn().id()));
        Ok(())
    }
}

impl OnDisconnect for Paired {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        self.hooks.push(Hook::Disconnected(conn.id(), why));
    }
}

/// Runs its connection phase once the connection is upgraded.
#[injectable]
struct Loose {
    hooks: ulo::Dep<Hooks>,
}

#[routes]
#[ulo_ws::gateway(path = "/loose", port = own)]
impl Loose {
    #[ulo_ws::message("noop")]
    fn noop(&self) {}
}

impl OnConnect for Loose {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        self.hooks.push(Hook::Connected(cx.conn().id()));
        Ok(())
    }
}

impl OnDisconnect for Loose {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        self.hooks.push(Hook::Disconnected(conn.id(), why));
    }
}

/// The header whose presence `NoRefuseHeader` refuses.
const REFUSE: &str = "x-refuse";

/// Refuses an upgrade carrying [`REFUSE`].
struct NoRefuseHeader;

impl Guard<WsConnect> for NoRefuseHeader {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().headers().get(REFUSE).is_none())
    }
}

/// Runs its connection phase once the connection is upgraded, one connection at a time, behind a
/// connect guard.
#[injectable]
struct Capped {
    hooks: ulo::Dep<Hooks>,
}

#[routes]
#[ulo_ws::gateway(path = "/capped", port = own, max_connections = 1, connect_guards(value = NoRefuseHeader))]
impl Capped {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

impl OnConnect for Capped {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        self.hooks.push(Hook::Connected(cx.conn().id()));
        Ok(())
    }
}

impl OnDisconnect for Capped {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        self.hooks.push(Hook::Disconnected(conn.id(), why));
    }
}

struct Root {
    joined: Joined,
    hooks: Hooks,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.joined.clone());
        m.value(self.hooks.clone());
        m.controller::<Plain>();
        m.controller::<Held>();
        m.controller::<Paired>();
        m.controller::<Loose>();
        m.controller::<Capped>();
    }
}

/// A server with no socket: the table it builds in `prepare` is handed to the test.
struct TableServer {
    table: Arc<Mutex<Option<GatewayTable>>>,
}

impl TableServer {
    fn table(&self) -> Option<GatewayTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ulo::Server for TableServer {
    type Transport = Ws;

    async fn prepare(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        let table = GatewayTable::own_port("TableServer", &mounted, &GatewayDefaults::default(), &mut failures);
        failures.into_result()?;
        *self.table.lock().unwrap_or_else(PoisonError::into_inner) = Some(table);
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        if let Some(table) = self.table() {
            table.start();
        }
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, token: DrainToken) {
        if let Some(table) = self.table() {
            table.drain(token).await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        if let Some(table) = self.table() {
            table.close().await;
        }
        Ok(())
    }
}

struct Running {
    table: GatewayTable,
    joined: Joined,
    hooks: Hooks,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start() -> Running {
    let slot = Arc::new(Mutex::new(None));
    let joined = Joined::default();
    let hooks = Hooks::default();
    let app: App<Serving> = App::builder(Root { joined: joined.clone(), hooks: hooks.clone() })
        .runtime(ulo_tokio::Tokio::current())
        .drain_timeout(Duration::from_secs(2))
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(TableServer { table: Arc::clone(&slot) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let table = slot.lock().unwrap_or_else(PoisonError::into_inner).clone().expect("the server built its table");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Running { table, joined, hooks, handle, serving }
}

impl Running {
    async fn stop(self) {
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("{what} did not happen within {WAIT:?}"))
}

/// A valid upgrade request's head for `path`, which `edit` may spoil.
fn head(path: &str, edit: impl FnOnce(&mut Parts)) -> Parts {
    let mut head = http::Request::builder()
        .method(Method::GET)
        .uri(path)
        .version(Version::HTTP_11)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", KEY)
        .body(())
        .expect("a request head")
        .into_parts()
        .0;
    edit(&mut head);
    head
}

/// The server's end handed to `switch.serve`, and the client's, already past the HTTP handshake.
async fn pipe() -> (DuplexStream, WebSocketStream<DuplexStream>) {
    let (server, client) = tokio::io::duplex(64 * 1024);
    (server, WebSocketStream::from_raw_socket(client, Role::Client, None).await)
}

#[tokio::test]
async fn a_server_without_hyper_serves_a_gateway_through_the_table() {
    let app = start().await;
    let Handshake::Switch(switch) = app.table.handshake(head("/plain", |_| {}), None).await else {
        panic!("a valid upgrade request to a gateway was refused");
    };
    let response = switch.response();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(response.headers().get(SEC_WEBSOCKET_ACCEPT).map(|value| value.as_bytes()), Some(ACCEPT.as_bytes()));
    let (server, mut client) = pipe().await;
    switch.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });

    let message = json!({ "event": "echo", "id": 1, "data": "over a pipe" }).to_string();
    client.send(Message::text(message)).await.expect("the message was sent");
    let reply = within("the reply", client.next()).await.expect("the connection ended").expect("a read failed");
    let reply: Value = serde_json::from_str(reply.to_text().expect("a text reply")).expect("a JSON reply");
    assert_eq!(reply, json!({ "id": 1, "data": "over a pipe" }));

    let _ = client.close(None).await;
    within("the server's end of the connection", async { while let Some(Ok(_)) = client.next().await {} }).await;
    app.stop().await;
}

/// What a head is, the head, the status it is refused with, and a header that refusal carries.
type Case = (&'static str, Parts, StatusCode, Option<(http::HeaderName, &'static str)>);

#[tokio::test]
async fn the_handshake_refuses_a_head_with_the_status_and_headers_rfc_6455_names() {
    let app = start().await;
    let cases: Vec<Case> = vec![
        ("no gateway at the path", head("/nowhere", |_| {}), StatusCode::NOT_FOUND, None),
        ("a POST", head("/plain", |head| head.method = Method::POST), StatusCode::METHOD_NOT_ALLOWED, Some((ALLOW, "GET"))),
        ("HTTP/1.0", head("/plain", |head| head.version = Version::HTTP_10), StatusCode::BAD_REQUEST, None),
        (
            "version 12",
            head("/plain", |head| drop(head.headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("12")))),
            StatusCode::UPGRADE_REQUIRED,
            Some((SEC_WEBSOCKET_VERSION, "13")),
        ),
        ("no upgrade token", head("/plain", |head| drop(head.headers.insert(UPGRADE, HeaderValue::from_static("h2c")))), StatusCode::BAD_REQUEST, None),
    ];
    for (what, head, status, header) in cases {
        let Handshake::Refuse(refusal) = app.table.handshake(head, None).await else {
            panic!("{what} was accepted");
        };
        assert_eq!(refusal.status(), status, "{what}");
        let response = refusal.into_response();
        assert_eq!(response.status(), status, "{what}");
        assert_eq!(
            response.headers().get(CONTENT_TYPE).map(|value| value.as_bytes()),
            Some(b"text/plain; charset=utf-8".as_slice()),
            "{what}"
        );
        assert!(!response.body().is_empty(), "{what}: the refusal carries no reason");
        if let Some((name, value)) = header {
            assert_eq!(response.headers().get(&name).map(|got| got.as_bytes()), Some(value.as_bytes()), "{what}: `{name}`");
        }
    }
    app.stop().await;
}

#[tokio::test]
async fn a_switch_dropped_unserved_takes_its_admitted_connection_out_of_the_rooms() {
    let app = start().await;
    let Handshake::Switch(switch) = app.table.handshake(head("/held", |_| {}), None).await else {
        panic!("the connection phase refused a valid upgrade request");
    };
    let conn = app.joined.0.lock().unwrap_or_else(PoisonError::into_inner).pop().expect("`OnConnect` ran in the handshake");
    assert_eq!(conn.rooms(), vec!["lobby".to_owned()], "the connection phase joined the room before the 101");
    drop(switch);
    // The connection leaves its rooms after its `on_disconnect`, on a task the drain waits for.
    app.stop().await;
    assert!(conn.rooms().is_empty(), "a connection never served is still in its rooms: {:?}", conn.rooms());
}

/// The 101 as a server writes it, to a client that has already gone: the write fails.
async fn write_the_101_to_a_departed_client() {
    let (mut server, client) = tokio::io::duplex(64);
    drop(client);
    let written = server.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").await;
    assert!(written.is_err(), "the 101 was written to a client that had gone");
}

/// The hooks one connection ran, read once the app has closed, whose drain waits for every
/// connection task, an unserved connection's `on_disconnect` included.
fn hooks_of(hooks: &Hooks, id: ConnId) -> Vec<Hook> {
    hooks
        .all()
        .into_iter()
        .filter(|hook| match hook {
            Hook::Connected(of) | Hook::Disconnected(of, _) => *of == id,
        })
        .collect()
}

#[tokio::test]
async fn a_switch_dropped_when_its_101_fails_to_write_runs_on_disconnect_once_as_lost() {
    let app = start().await;
    let hooks = app.hooks.clone();
    let Handshake::Switch(switch) = app.table.handshake(head("/paired", |_| {}), None).await else {
        panic!("the connection phase refused a valid upgrade request");
    };
    let id = hooks.last_connected();
    write_the_101_to_a_departed_client().await;
    drop(switch);
    app.stop().await;
    assert_eq!(hooks_of(&hooks, id), vec![Hook::Connected(id), Hook::Disconnected(id, DisconnectReason::Lost)]);
}

#[tokio::test]
async fn an_upgrade_that_fails_after_the_switch_is_served_runs_on_disconnect_once_as_lost() {
    let app = start().await;
    let hooks = app.hooks.clone();
    let Handshake::Switch(switch) = app.table.handshake(head("/paired", |_| {}), None).await else {
        panic!("the connection phase refused a valid upgrade request");
    };
    let id = hooks.last_connected();
    // hyper's shape: the upgrade future fails when the 101 cannot be written.
    switch.serve(async move {
        write_the_101_to_a_departed_client().await;
        Err::<Upgraded, BoxError>("the 101 could not be written".into())
    });
    app.stop().await;
    assert_eq!(hooks_of(&hooks, id), vec![Hook::Connected(id), Hook::Disconnected(id, DisconnectReason::Lost)]);
}

#[tokio::test]
async fn a_failed_101_before_the_connection_phase_runs_no_hook() {
    let app = start().await;
    let hooks = app.hooks.clone();
    let Handshake::Switch(dropped) = app.table.handshake(head("/loose", |_| {}), None).await else {
        panic!("a valid upgrade request was refused");
    };
    let Handshake::Switch(failed) = app.table.handshake(head("/loose", |_| {}), None).await else {
        panic!("a valid upgrade request was refused");
    };
    write_the_101_to_a_departed_client().await;
    drop(dropped);
    failed.serve(async move { Err::<Upgraded, BoxError>("the 101 could not be written".into()) });
    app.stop().await;
    assert_eq!(hooks.all(), Vec::new(), "a hook ran for a connection whose connection phase never began");
}

/// The connection holding `path`'s one slot, shown by an answered message, and its id.
async fn hold_the_slot(app: &Running, path: &str) -> (WebSocketStream<DuplexStream>, ConnId) {
    let Handshake::Switch(first) = app.table.handshake(head(path, |_| {}), None).await else {
        panic!("{path}: a valid upgrade request was refused");
    };
    let (server, mut holder) = pipe().await;
    first.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });
    let message = json!({ "event": "echo", "id": 1, "data": "holding" }).to_string();
    holder.send(Message::text(message)).await.expect("the message was sent");
    within("the holder's reply", holder.next()).await.expect("the connection ended").expect("a read failed");
    (holder, app.hooks.last_connected())
}

/// The close code `ws`'s server sends first, read to the end of the connection, which sends the
/// client's answering Close the server waits for.
async fn close_code(what: &str, ws: &mut WebSocketStream<DuplexStream>) -> u16 {
    let close = within(what, ws.next()).await.expect("the connection ended").expect("a read failed");
    let Message::Close(Some(frame)) = close else { panic!("{what}: expected a Close frame, got {close:?}") };
    within(what, async { while let Some(Ok(_)) = ws.next().await {} }).await;
    u16::from(frame.code)
}

/// Hangs up `holder` and waits for its end.
async fn hang_up(mut holder: WebSocketStream<DuplexStream>) {
    let _ = holder.close(None).await;
    within("the holder's end of the connection", async { while let Some(Ok(_)) = holder.next().await {} }).await;
}

#[tokio::test]
async fn a_connection_over_max_connections_admitted_in_the_handshake_is_closed_1013_with_no_hook() {
    let app = start().await;
    let hooks = app.hooks.clone();
    let (holder, holding) = hold_the_slot(&app, "/paired").await;

    let Handshake::Switch(second) = app.table.handshake(head("/paired", |_| {}), None).await else {
        panic!("the connection phase refused a valid upgrade request: capacity is refused with 1013 after the 101");
    };
    let (server, mut over) = pipe().await;
    second.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });
    assert_eq!(close_code("the second connection's close", &mut over).await, 1013);
    hang_up(holder).await;
    app.stop().await;
    let all = hooks.all();
    assert_eq!(
        all,
        vec![Hook::Connected(holding), Hook::Disconnected(holding, DisconnectReason::ClientClose { code: 1005, reason: String::new() })],
        "a hook ran for the connection refused for capacity"
    );
}

#[tokio::test]
async fn a_full_gateway_s_connect_guard_refuses_first_and_capacity_runs_no_hook() {
    let app = start().await;
    let hooks = app.hooks.clone();
    let (holder, holding) = hold_the_slot(&app, "/capped").await;

    // Refused by the guard, which decides before a slot is asked for.
    let Handshake::Switch(guarded) = app.table.handshake(head("/capped", |head| {
        head.headers.insert(REFUSE, HeaderValue::from_static("yes"));
    }), None).await else {
        panic!("`refuse = close` switches every valid upgrade request");
    };
    let (server, mut refused) = pipe().await;
    guarded.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });
    assert_eq!(close_code("the guarded connection's close", &mut refused).await, 1008, "the connect guard's refusal");

    // Admitted by the guard, then refused for capacity before `OnConnect`.
    let Handshake::Switch(over) = app.table.handshake(head("/capped", |_| {}), None).await else {
        panic!("`refuse = close` switches every valid upgrade request");
    };
    let (server, mut full) = pipe().await;
    over.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });
    assert_eq!(close_code("the connection over the limit", &mut full).await, 1013);

    hang_up(holder).await;
    app.stop().await;
    assert_eq!(
        hooks.all(),
        vec![Hook::Connected(holding), Hook::Disconnected(holding, DisconnectReason::ClientClose { code: 1005, reason: String::new() })],
        "a hook ran for a connection refused by its guard or for capacity"
    );
}

/// The table's own refusal at the drain: the one a standalone server writes, the HTTP server's
/// port answering with that server's own 503 first. It comes before the connection phase, which
/// runs for no gateway once the drain has begun.
#[tokio::test]
async fn a_handshake_once_the_drain_has_begun_is_refused_503_before_the_connection_phase() {
    let app = start().await;
    let (table, hooks) = (app.table.clone(), app.hooks.clone());
    app.stop().await;
    for path in ["/plain", "/paired"] {
        let Handshake::Refuse(refusal) = table.handshake(head(path, |_| {}), None).await else {
            panic!("{path}: a handshake once the drain had begun was accepted");
        };
        assert_eq!(refusal.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(refusal.reason(), "the server is shutting down", "{path}");
    }
    assert_eq!(hooks.all(), Vec::new(), "the connection phase ran once the drain had begun");
}
