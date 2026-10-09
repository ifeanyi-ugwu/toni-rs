//! `ulo-ws`'s serving surface without hyper or a socket: a test server builds a `GatewayTable` in
//! `prepare`, answers request heads with its handshake decision, and drives each accepted
//! connection through `Switch::serve` on one end of an in-memory pipe, a WebSocket client
//! speaking on the other. This is the whole of what a server on another HTTP stack or runtime
//! writes around the table.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::header::{ALLOW, CONTENT_TYPE, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_VERSION, UPGRADE};
use http::request::Parts;
use http::{Method, StatusCode, Version};
use serde_json::{Value, json};
use tokio::io::DuplexStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, BoxError, DrainToken, Module, ModuleDef, ModuleIdentity, Mounted, Signal, injectable, routes};
use ulo_http::Upgraded;
use ulo_transport::prepare::Failures;
use ulo_ws::{ConnectCx, ConnectRefused, Connection, GatewayDefaults, GatewayTable, Handshake, OnConnect, Ws};

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

struct Root {
    joined: Joined,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.joined.clone());
        m.controller::<Plain>();
        m.controller::<Held>();
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
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start() -> Running {
    let slot = Arc::new(Mutex::new(None));
    let joined = Joined::default();
    let app: App<Serving> = App::builder(Root { joined: joined.clone() })
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
    Running { table, joined, handle, serving }
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
    assert!(conn.rooms().is_empty(), "a connection never served is still in its rooms: {:?}", conn.rooms());
    app.stop().await;
}
