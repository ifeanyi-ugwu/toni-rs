//! `#[ulo_ws::gateway]` and `#[ulo_ws::message]` expanded against the runtime: a gateway with a
//! handler per reply kind, a session built by `session_with`, a connect guard and `OnConnect`, and
//! a generic gateway beside it, both served on their own port and driven through a WebSocket
//! client.

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_transport::Classify;
use ulo_ws::{ConnectCx, ConnectRefused, Frame, OnConnect, Payload, Reply, Session, UpgradeHead, WsConnect};

/// The upgrade header the connect guard admits on, and the value it admits.
const TOKEN_HEADER: &str = "x-token";
const TOKEN: &str = "open";

/// The upgrade header the session factory reads the visitor's name from.
const USER_HEADER: &str = "x-user";

/// The visitor `on_connect` refuses, and the close code it refuses with.
const BANNED: &str = "mallory";
const BANNED_CODE: u16 = 4000;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How long the client waits for one frame before failing the test.
const FRAME_WAIT: Duration = Duration::from_secs(5);

/// Admits an upgrade carrying `x-token: open`. Bound in the module, since a guard named by type
/// is resolved from the container.
#[injectable]
struct TokenGuard;

impl Guard<WsConnect> for TokenGuard {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().headers().get(TOKEN_HEADER).is_some_and(|token| token == TOKEN))
    }
}

/// Per-connection state: the name the upgrade request carried and the messages counted so far.
struct Visitor {
    name: String,
    seen: AtomicUsize,
}

#[derive(Deserialize)]
struct Greet {
    name: String,
}

#[derive(Serialize)]
struct Greeting {
    text: String,
}

/// A handler's own failure, classified `conflict`.
#[derive(Debug, Classify)]
#[classify(conflict)]
struct Taken;

impl fmt::Display for Taken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the name is taken")
    }
}

impl Error for Taken {}

fn greeting(greet: &Greet) -> Result<Greeting, Taken> {
    if greet.name == "taken" { Err(Taken) } else { Ok(Greeting { text: format!("hello, {}", greet.name) }) }
}

#[injectable]
struct Chat;

#[routes]
#[ulo_ws::gateway(
    path = "/chat",
    port = own,
    connect_guards(TokenGuard),
    session_with = |head: Dep<UpgradeHead>| Visitor {
        name: head.headers().get(USER_HEADER).and_then(|name| name.to_str().ok()).unwrap_or("anonymous").to_owned(),
        seen: AtomicUsize::new(0),
    }
)]
impl Chat {
    #[ulo_ws::message("unit")]
    async fn unit(&self) {}

    #[ulo_ws::message("value")]
    fn value(&self, greet: Payload<Greet>) -> Greeting {
        Greeting { text: format!("hello, {}", greet.0.name) }
    }

    #[ulo_ws::message("frame")]
    fn frame(&self) -> Frame {
        Frame::text("[1,2,3]")
    }

    #[ulo_ws::message("reply")]
    async fn reply(&self) -> Reply {
        Reply::One(Frame::text("\"by hand\""))
    }

    #[ulo_ws::message("count")]
    async fn count(&self, up_to: Payload<u32>) -> impl Stream<Item = Result<u32, Taken>> {
        stream::iter((1..=up_to.0).map(Ok))
    }

    #[ulo_ws::message("greet")]
    async fn greet(&self, greet: Payload<Greet>) -> Result<Greeting, Taken> {
        greeting(&greet.0)
    }

    #[ulo_ws::message("greet.unit")]
    async fn greet_unit(&self, greet: Payload<Greet>) -> Result<(), Taken> {
        greeting(&greet.0).map(drop)
    }

    #[ulo_ws::message("greet.frame")]
    async fn greet_frame(&self, greet: Payload<Greet>) -> Result<Frame, Taken> {
        greeting(&greet.0).map(|greeting| Frame::text(format!("{:?}", greeting.text)))
    }

    #[ulo_ws::message("greet.reply")]
    async fn greet_reply(&self, greet: Payload<Greet>) -> Result<Reply, Taken> {
        greeting(&greet.0).map(|_| Reply::None)
    }

    /// Fails before streaming for `0`; otherwise streams `1..=n` and fails the item after.
    #[ulo_ws::message("count.result")]
    async fn count_result(&self, up_to: Payload<u32>) -> Result<impl Stream<Item = Result<u32, Taken>>, Taken> {
        if up_to.0 == 0 {
            return Err(Taken);
        }
        Ok(stream::iter((1..=up_to.0).map(Ok).chain([Err(Taken)])))
    }

    #[ulo_ws::message("visit")]
    async fn visit(&self, visitor: Session<Visitor>) -> Value {
        let seen = visitor.seen.fetch_add(1, Ordering::SeqCst) + 1;
        json!({ "name": visitor.name, "seen": seen })
    }
}

/// Refuses the banned visitor after the connect guard admitted the upgrade.
impl OnConnect for Chat {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        if cx.head().headers().get(USER_HEADER).is_some_and(|name| name == BANNED) {
            return Err(ConnectRefused::code(BANNED_CODE, "banned")
                .unwrap_or_else(|error| panic!("{BANNED_CODE} is a close code a frame carries: {error}")));
        }
        Ok(())
    }
}

/// The word `Echo<W>` prefixes, a type parameter of the gateway.
trait Word: Send + Sync + 'static {
    const WORD: &'static str;
}

struct Hi;

impl Word for Hi {
    const WORD: &'static str = "hi";
}

/// A generic gateway: its `GatewayConfig` impl, its hook probes and its reply probe sit in a
/// generic impl.
#[injectable]
struct Echo<W: Word> {
    #[injectable(default)]
    _word: PhantomData<W>,
}

#[routes]
#[ulo_ws::gateway(path = "/echo", port = own)]
impl<W: Word> Echo<W> {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        format!("{} {}", W::WORD, text.0)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<TokenGuard>();
        m.controller::<Chat>();
        m.controller::<Echo<Hi>>();
    }
}

/// The app listening on an ephemeral port, serving until the test ends.
struct Running {
    addr: SocketAddr,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start() -> Running {
        let app: App<Serving> = App::builder(Root)
            .timer(ulo_tokio::Timer)
            .wire()
            .unwrap_or_else(|error| panic!("the gateway app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the gateway app did not connect: {error}"))
            .bind(ulo_ws::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the gateway app did not listen: {error}"));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr, handle, serving }
    }

    async fn connect(&self, headers: &[(&'static str, &'static str)]) -> Socket {
        self.connect_to("/chat", headers).await
    }

    async fn connect_to(&self, path: &str, headers: &[(&'static str, &'static str)]) -> Socket {
        let mut request = format!("ws://{}{path}", self.addr)
            .into_client_request()
            .unwrap_or_else(|error| panic!("the client request did not build: {error}"));
        for (name, value) in headers {
            request
                .headers_mut()
                .insert(*name, value.parse().unwrap_or_else(|error| panic!("bad header value: {error}")));
        }
        let (socket, response) = tokio_tungstenite::connect_async(request)
            .await
            .unwrap_or_else(|error| panic!("the WebSocket handshake failed: {error}"));
        assert_eq!(response.status(), 101, "expected the upgrade to switch protocols");
        socket
    }

    /// A connection the connect guard admits, as visitor `ada`.
    async fn admitted(&self) -> Socket {
        self.connect(&[(TOKEN_HEADER, TOKEN), (USER_HEADER, "ada")]).await
    }

    /// Closes the app. A client still connected gets 1001 and is waited for until it answers or
    /// the drain timeout passes, so each test hangs up first.
    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

/// Closes the client's side, answering a close the server already sent.
async fn hang_up(mut socket: Socket) {
    let _ = socket.close(None).await;
}

/// Sends `{"event", "id": 1, "data"}`, `data` omitted when `None`.
async fn send(socket: &mut Socket, event: &str, data: Option<Value>) {
    let mut message = json!({ "event": event, "id": 1 });
    if let Some(data) = data {
        message["data"] = data;
    }
    socket
        .send(Message::text(message.to_string()))
        .await
        .unwrap_or_else(|error| panic!("sending `{event}` failed: {error}"));
}

/// The next data message, parsed as JSON.
async fn next(socket: &mut Socket) -> Value {
    match tokio::time::timeout(FRAME_WAIT, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("the server sent non-JSON text {text:?}: {error}")),
        Ok(other) => panic!("expected a text message, got {other:?}"),
        Err(_) => panic!("no message arrived within {FRAME_WAIT:?}"),
    }
}

/// Sends one message and reads frames until the answer ends: one `data` frame, a `complete`, or
/// an `error`, and for a stream every `data` frame up to its end.
async fn exchange(socket: &mut Socket, event: &str, data: Option<Value>) -> Vec<Value> {
    send(socket, event, data).await;
    let mut frames = Vec::new();
    loop {
        let frame = next(socket).await;
        let ends = frame.get("complete").is_some() || frame.get("error").is_some();
        frames.push(frame);
        if ends {
            return frames;
        }
    }
}

/// One `data` answer, read without waiting for a `complete` that a single answer never sends.
async fn single(socket: &mut Socket, event: &str, data: Option<Value>) -> Value {
    send(socket, event, data).await;
    next(socket).await
}

fn conflict() -> Value {
    json!({ "id": 1, "error": { "kind": "conflict", "message": "the name is taken", "details": [] } })
}

#[tokio::test]
async fn unit_reply_completes_the_message() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    assert_eq!(exchange(&mut socket, "unit", None).await, vec![json!({ "id": 1, "complete": true })]);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn serializable_reply_is_the_data() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    let answer = single(&mut socket, "value", Some(json!({ "name": "ada" }))).await;
    assert_eq!(answer, json!({ "id": 1, "data": { "text": "hello, ada" } }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn frame_reply_is_the_data_as_written() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    assert_eq!(single(&mut socket, "frame", None).await, json!({ "id": 1, "data": [1, 2, 3] }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn hand_built_reply_is_written_as_it_stands() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    assert_eq!(single(&mut socket, "reply", None).await, json!({ "id": 1, "data": "by hand" }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn stream_reply_writes_each_item_then_completes() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    let frames = exchange(&mut socket, "count", Some(json!(3))).await;
    let expected = vec![
        json!({ "id": 1, "data": 1 }),
        json!({ "id": 1, "data": 2 }),
        json!({ "id": 1, "data": 3 }),
        json!({ "id": 1, "complete": true }),
    ];
    assert_eq!(frames, expected);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn result_reply_answers_ok_as_its_value_and_err_as_the_error_envelope() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;

    let ok = single(&mut socket, "greet", Some(json!({ "name": "ada" }))).await;
    assert_eq!(ok, json!({ "id": 1, "data": { "text": "hello, ada" } }));
    assert_eq!(exchange(&mut socket, "greet", Some(json!({ "name": "taken" }))).await, vec![conflict()]);

    assert_eq!(
        exchange(&mut socket, "greet.unit", Some(json!({ "name": "ada" }))).await,
        vec![json!({ "id": 1, "complete": true })]
    );
    assert_eq!(exchange(&mut socket, "greet.unit", Some(json!({ "name": "taken" }))).await, vec![conflict()]);

    let frame = single(&mut socket, "greet.frame", Some(json!({ "name": "ada" }))).await;
    assert_eq!(frame, json!({ "id": 1, "data": "hello, ada" }));
    assert_eq!(exchange(&mut socket, "greet.frame", Some(json!({ "name": "taken" }))).await, vec![conflict()]);

    assert_eq!(
        exchange(&mut socket, "greet.reply", Some(json!({ "name": "ada" }))).await,
        vec![json!({ "id": 1, "complete": true })]
    );
    assert_eq!(exchange(&mut socket, "greet.reply", Some(json!({ "name": "taken" }))).await, vec![conflict()]);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn result_of_a_stream_fails_before_its_first_item_or_on_an_err_item() {
    let app = Running::start().await;
    let mut socket = app.admitted().await;
    assert_eq!(exchange(&mut socket, "count.result", Some(json!(0))).await, vec![conflict()]);
    let frames = exchange(&mut socket, "count.result", Some(json!(2))).await;
    assert_eq!(frames, vec![json!({ "id": 1, "data": 1 }), json!({ "id": 1, "data": 2 }), conflict()]);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn session_with_builds_one_session_per_connection_from_the_upgrade_request() {
    let app = Running::start().await;
    let mut first = app.admitted().await;
    assert_eq!(single(&mut first, "visit", None).await, json!({ "id": 1, "data": { "name": "ada", "seen": 1 } }));
    assert_eq!(single(&mut first, "visit", None).await, json!({ "id": 1, "data": { "name": "ada", "seen": 2 } }));

    let mut second = app.connect(&[(TOKEN_HEADER, TOKEN), (USER_HEADER, "grace")]).await;
    assert_eq!(single(&mut second, "visit", None).await, json!({ "id": 1, "data": { "name": "grace", "seen": 1 } }));
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}

/// The close code the server ends `socket` with, skipping any text before the Close frame.
async fn closed_with(socket: &mut Socket) -> Option<CloseCode> {
    let close = loop {
        match tokio::time::timeout(FRAME_WAIT, socket.next()).await {
            Ok(Some(Ok(Message::Close(frame)))) => break frame,
            Ok(Some(Ok(Message::Text(_)))) => continue,
            Ok(other) => panic!("expected the refused connection to close, got {other:?}"),
            Err(_) => panic!("the refused connection was not closed within {FRAME_WAIT:?}"),
        }
    };
    close.map(|frame| frame.code)
}

#[tokio::test]
async fn connect_guard_refusal_closes_the_connection_with_policy_violation() {
    let app = Running::start().await;
    let mut socket = app.connect(&[(USER_HEADER, "ada")]).await;
    assert_eq!(closed_with(&mut socket).await, Some(CloseCode::Policy), "expected 1008 for a refused connect guard");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn on_connect_refusal_closes_with_its_own_code() {
    let app = Running::start().await;
    let mut socket = app.connect(&[(TOKEN_HEADER, TOKEN), (USER_HEADER, BANNED)]).await;
    assert_eq!(closed_with(&mut socket).await, Some(CloseCode::from(BANNED_CODE)));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn generic_gateway_answers_its_message() {
    let app = Running::start().await;
    let mut socket = app.connect_to("/echo", &[]).await;
    assert_eq!(single(&mut socket, "echo", Some(json!("there"))).await, json!({ "id": 1, "data": "hi there" }));
    hang_up(socket).await;
    app.stop().await;
}
