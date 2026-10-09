//! Every task the standalone server's connections need is spawned on the app's runtime: broadcast
//! delivery and each gateway's `AfterInit` once the server is bound, then one task per connection
//! and one per message; only the accept loop and the HTTP/1.1 handshake run on tokio. The app's
//! runtime counts what it is handed.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use ulo::{BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Spawn, TaskHandle, Timer, injectable, routes};
use ulo_tokio::Tokio;
use ulo_ws::{AfterInit, GatewayRef, WsModule};

use support::{Record, Running, hang_up, next_json, send_json};

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

/// Each gateway's `AfterInit` call, by path.
#[derive(Clone)]
struct Inits(Record<String>);

#[injectable]
struct Own {
    inits: Dep<Inits>,
}

#[routes]
#[ulo_ws::gateway(path = "/own", port = own)]
impl Own {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

impl AfterInit for Own {
    async fn after_init(&self, gw: GatewayRef) {
        self.inits.0.push(gw.path().to_owned());
    }
}

/// `WsModule` and the one gateway, `Own`.
struct Root {
    inits: Inits,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.inits.clone());
        m.controller::<Own>();
    }
}

/// Starts the app on a counting runtime, waits for its gateway's `AfterInit`, then connects and
/// exchanges one message, checking the tasks spawned at each step.
async fn spawns_on_the_app_s_runtime(server: impl ulo::Server, path: &str) {
    let runtime = Counting { tokio: Tokio::current(), spawned: Arc::default() };
    let inits = Inits(Record::new());
    let app = Running::start_on(Root { inits: inits.clone() }, server, runtime.clone()).await;
    assert_eq!(inits.0.at_least(1, "the gateway's `AfterInit`").await, vec![path.to_owned()]);
    assert_eq!(runtime.spawned(), 2, "broadcast delivery and `AfterInit`, spawned once the server was bound");

    let mut socket = app.connect(path, &[]).await;
    assert_eq!(runtime.spawned(), 3, "the connection's task, spawned with its 101");
    send_json(&mut socket, &json!({ "event": "ping", "id": 1 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "pong" }));
    assert_eq!(runtime.spawned(), 4, "the message's task");

    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn the_standalone_server_spawns_every_task_on_the_app_s_runtime() {
    spawns_on_the_app_s_runtime(ulo_ws_hyper::Server::new("127.0.0.1:0"), "/own").await;
}
