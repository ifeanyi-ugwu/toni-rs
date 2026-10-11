//! Every task `ulo-ws` starts is spawned on the app's runtime: broadcast delivery and each
//! gateway's `AfterInit` once the server is bound, then one task per connection and one per
//! message, here through the hand-off on the HTTP server's port, whose own accept loop and
//! connection tasks the app's runtime runs too; `ulo-ws-hyper`'s `runtime.rs` holds the standalone
//! server to the same counts. The app's runtime counts what it is handed.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use ulo::{BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Spawn, TaskHandle, Timer, injectable, routes};
use ulo_tokio::Tokio;
use ulo_ws::{AfterInit, GatewayRef, WsModule};

use support::{Record, Running, hang_up, next_json, send_json, within};

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
struct Shared {
    inits: Dep<Inits>,
}

#[routes]
#[ulo_ws::gateway(path = "/shared")]
impl Shared {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

impl AfterInit for Shared {
    async fn after_init(&self, gw: GatewayRef) {
        self.inits.0.push(gw.path().to_owned());
    }
}

/// `WsModule` and the one gateway, `Shared`.
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
        m.controller::<Shared>();
    }
}

/// Starts the app on a counting runtime, waits for its gateway's `AfterInit`, then connects and
/// exchanges one message, checking the tasks spawned at each step.
async fn spawns_on_the_app_s_runtime(server: impl ulo::Server, path: &str) {
    let runtime = Counting { tokio: Tokio::current(), spawned: Arc::default() };
    let inits = Inits(Record::new());
    let app = Running::start_on(Root { inits: inits.clone() }, server, runtime.clone()).await;
    assert_eq!(inits.0.at_least(1, "the gateway's `AfterInit`").await, vec![path.to_owned()]);
    // The accept loop starts with `serve`, which runs on its own task beside the bound app, so it
    // is waited for rather than read at once.
    within("the accept loop's task", async {
        while runtime.spawned() < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(runtime.spawned(), 3, "broadcast delivery, `AfterInit` and the server's accept loop");

    let mut socket = app.connect(path, &[]).await;
    assert_eq!(runtime.spawned(), 5, "the HTTP connection's task, spawned at the accept, and the WebSocket connection's, spawned with its 101");
    send_json(&mut socket, &json!({ "event": "ping", "id": 1 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "pong" }));
    assert_eq!(runtime.spawned(), 6, "the message's task");

    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn the_hand_off_spawns_every_task_on_the_app_s_runtime() {
    spawns_on_the_app_s_runtime(ulo_http_hyper::Server::new("127.0.0.1:0"), "/shared").await;
}
