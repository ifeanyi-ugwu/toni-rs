//! A gateway on the HTTP transport, served inside axum through `WsModule`'s upgrade hand-off. axum
//! declares `upgrades`, so the upgrade must reach the gateway: the hand-off depends on the upgrade
//! `Embed::take_upgrade` takes out of the request, which the default leaves behind.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tower::Layer;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http_axum::{Embedded, HostLayer};
use ulo_ws::WsModule;

/// How long any one step may take.
const PATIENCE: Duration = Duration::from_secs(5);

#[injectable]
struct Echo;

#[routes]
#[ulo_ws::gateway(path = "/ws")]
impl Echo {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Echo>();
    }
}

#[tokio::test]
async fn a_gateway_on_the_http_transport_answers_inside_axum() {
    let server = Embedded::new();
    let embedded = server.handle();
    let app = App::builder(Root)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(server)
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen inside axum: {error}"));
    let router = Router::new().fallback_service(HostLayer.layer(embedded.service()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the host binds a port");
    let addr = listener.local_addr().expect("the host's listener has an address");
    let serve = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>());
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let signal = async move {
            let _ = stopped.await;
            Signal::new("test")
        };
        let _ = ulo_http_axum::run(app, &embedded, serve, signal).await;
    });

    let answer = tokio::time::timeout(PATIENCE, async {
        let stream = TcpStream::connect(addr).await.expect("the client reaches the host");
        let (mut socket, response) = tokio_tungstenite::client_async(format!("ws://{addr}/ws"), stream)
            .await
            .unwrap_or_else(|error| panic!("axum declares `upgrades` and the gateway's upgrade did not switch protocols: {error}"));
        assert_eq!(response.status(), 101);
        socket.send(Message::text(json!({ "event": "ping", "id": 1 }).to_string())).await.expect("the client sends");
        let Some(Ok(Message::Text(text))) = socket.next().await else {
            panic!("the gateway did not answer with a text frame");
        };
        serde_json::from_str::<Value>(&text).expect("the answer is JSON")
    })
    .await
    .expect("the gateway did not answer in time");
    assert_eq!(answer, json!({ "id": 1, "data": "pong" }));

    let _ = stop.send(());
    let _ = serving.await;
}
