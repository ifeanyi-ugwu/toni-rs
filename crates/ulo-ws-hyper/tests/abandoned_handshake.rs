//! A client that goes while a `refuse = handshake` gateway's connection phase runs. hyper ends a
//! request whose client half-closes before it is answered and drops its response future; the
//! connection phase still runs to its end, and the connection it admitted gets `on_disconnect`
//! with `Lost` once and leaves its rooms, as every connection whose `OnConnect` ran does.

mod support;

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use ulo::{Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_ws::{ConnectCx, ConnectRefused, Connection, DisconnectReason, OnConnect, OnDisconnect};

use support::{Record, Running, within};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Hook {
    Connected,
    Disconnected(DisconnectReason),
}

/// What the test holds of the gateway: its hooks, the connections it admitted, and the gate its
/// `OnConnect` waits at.
#[derive(Clone)]
struct Gate {
    hooks: Record<Hook>,
    admitted: Record<Connection>,
    entered: Arc<Notify>,
    open: Arc<Notify>,
}

#[injectable]
struct Gated {
    gate: Dep<Gate>,
}

#[routes]
#[ulo_ws::gateway(path = "/gated", port = own, refuse = handshake)]
impl Gated {
    #[ulo_ws::message("noop")]
    fn noop(&self) {}
}

impl OnConnect for Gated {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join("lobby").await;
        self.gate.admitted.push(cx.conn().clone());
        self.gate.hooks.push(Hook::Connected);
        self.gate.entered.notify_one();
        self.gate.open.notified().await;
        Ok(())
    }
}

impl OnDisconnect for Gated {
    async fn on_disconnect(&self, _conn: &Connection, why: DisconnectReason) {
        self.gate.hooks.push(Hook::Disconnected(why));
    }
}

struct Root(Gate);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Gated>();
    }
}

#[tokio::test]
async fn a_client_gone_while_on_connect_runs_gets_on_disconnect_once_as_lost() {
    let gate = Gate { hooks: Record::new(), admitted: Record::new(), entered: Arc::new(Notify::new()), open: Arc::new(Notify::new()) };
    let app = Running::start(Root(gate.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut stream = TcpStream::connect(app.addr).await.unwrap_or_else(|error| panic!("connecting to {} failed: {error}", app.addr));
    let request = format!(
        "GET /gated HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        app.addr
    );
    stream.write_all(request.as_bytes()).await.unwrap_or_else(|error| panic!("writing the upgrade request failed: {error}"));
    within("`OnConnect`", gate.entered.notified()).await;

    // The client half-closes; hyper ends the unanswered request, which the end of the stream shows.
    stream.shutdown().await.unwrap_or_else(|error| panic!("half-closing the connection failed: {error}"));
    let mut answer = Vec::new();
    within("the server's end of the connection", stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|error| panic!("reading the server's end failed: {error}"));
    assert!(answer.is_empty(), "the server answered a client that had gone: {:?}", String::from_utf8_lossy(&answer));

    gate.open.notify_one();
    let hooks = gate.hooks.at_least(2, "`on_disconnect`").await;
    app.stop().await;
    assert_eq!(hooks, vec![Hook::Connected, Hook::Disconnected(DisconnectReason::Lost)]);
    assert_eq!(gate.hooks.snapshot().len(), 2, "a hook ran again by the app's close: {:?}", gate.hooks.snapshot());
    let rooms: Vec<Vec<String>> = gate.admitted.snapshot().iter().map(Connection::rooms).collect();
    assert_eq!(rooms, vec![Vec::<String>::new()], "the connection is still in its rooms");
}
