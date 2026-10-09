//! The gateway's own tasks run on the app's runtime: the `connection_init_timeout` watch each
//! connection starts, and the task each `subscribe` streams in. The app's runtime counts what it
//! is handed; before either, `ulo-ws` has spawned four: broadcast delivery, the gateway's
//! `AfterInit` (the trait's own, which does nothing), the connection, and the gateway's inbox.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt, stream};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{
    App, AppHandle, Bound, BoxFuture, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, Spawn, TaskHandle, Timer,
};
use ulo_graphql::{BoxStream, Engine, GqlRequest, GqlResponse};
use ulo_graphql_ws::GraphqlWs;
use ulo_graphql_ws::__private::InitTimeout;
use ulo_tokio::Tokio;
use ulo_ws::WsModule;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);

/// Tokio, counting each task it is handed.
#[derive(Clone)]
struct Counting {
    tokio: Tokio,
    spawned: Arc<AtomicUsize>,
}

impl Counting {
    fn spawned(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

impl Timer for Counting {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.tokio.sleep(d)
    }

    fn now(&self) -> Instant {
        self.tokio.now()
    }
}

impl Spawn for Counting {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.tokio.spawn(fut)
    }
}

/// Answers every subscription with two events.
struct Ticks;

impl Engine for Ticks {
    fn execute(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        Box::pin(async { GqlResponse::executed(Value::Null, Vec::new()) })
    }

    fn subscribe(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        Box::pin(stream::iter((1..=2).map(|tick| GqlResponse::executed(json!({ "tick": tick }), Vec::new()))))
    }

    fn sdl(&self) -> String {
        String::new()
    }

    fn playground_html(&self, _endpoint: &str, _subscriptions: Option<&str>) -> Option<String> {
        None
    }
}

struct Root {
    init_timeout: Duration,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(Ticks).also_as::<dyn Engine>(|engine| engine);
        m.value(InitTimeout(Bound::After(self.init_timeout)));
        m.controller::<GraphqlWs>().at("/graphql");
    }
}

struct Running {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start(runtime: Counting, init_timeout: Duration) -> Running {
    let app: App<Serving> = App::builder(Root { init_timeout })
        .runtime(runtime)
        .drain_timeout(Duration::from_secs(2))
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addr = app.addresses()[0].addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Running { addr, handle, serving }
}

impl Running {
    async fn connect(&self) -> Socket {
        let mut request = format!("ws://{}/graphql", self.addr).into_client_request().expect("a client request");
        request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static("graphql-transport-ws"));
        let (socket, _) = within("the upgrade", tokio_tungstenite::connect_async(request)).await.expect("the upgrade succeeded");
        socket
    }

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

/// Closes the client's side, or answers a Close the server sent, and reads until the server ends
/// the connection.
async fn hang_up(mut socket: Socket) {
    let _ = socket.close(None).await;
    within("the server's end of the connection", async { while let Some(Ok(_)) = socket.next().await {} }).await;
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.expect("the message was sent");
}

async fn next(socket: &mut Socket) -> Value {
    let message = within("the next message", socket.next()).await.expect("the connection ended").expect("a read failed");
    match message {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("a JSON message"),
        other => panic!("expected a text message, got {other:?}"),
    }
}

#[tokio::test]
async fn a_subscription_streams_on_the_app_s_runtime() {
    let runtime = Counting { tokio: Tokio::current(), spawned: Arc::default() };
    let app = start(runtime.clone(), Duration::from_secs(30)).await;
    let mut socket = app.connect().await;
    send(&mut socket, json!({ "type": "connection_init" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "connection_ack" }));
    assert_eq!(runtime.spawned(), 5, "`ulo-ws`'s four and the `connection_init_timeout` watch");

    send(&mut socket, json!({ "type": "subscribe", "id": "s", "payload": { "query": "subscription { tick }" } })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "next", "id": "s", "payload": { "data": { "tick": 1 } } }));
    assert_eq!(next(&mut socket).await, json!({ "type": "next", "id": "s", "payload": { "data": { "tick": 2 } } }));
    assert_eq!(next(&mut socket).await, json!({ "type": "complete", "id": "s" }));
    assert_eq!(runtime.spawned(), 6, "the subscription's task");

    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn the_init_timeout_closes_the_connection_from_the_app_s_runtime() {
    let runtime = Counting { tokio: Tokio::current(), spawned: Arc::default() };
    let app = start(runtime.clone(), Duration::from_millis(100)).await;
    let mut socket = app.connect().await;
    let close = loop {
        match within("the close", socket.next()).await {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => {}
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    let frame = close.expect("the close frame carries a code");
    assert_eq!(frame.code, CloseCode::from(4408));
    assert_eq!(frame.reason.as_str(), "Connection initialisation timeout");
    assert_eq!(runtime.spawned(), 5, "`ulo-ws`'s four and the `connection_init_timeout` watch");
    hang_up(socket).await;
    app.stop().await;
}
