//! An adapter built on a thread with no tokio runtime current, and given none with `with_handle`,
//! fails its `prepare` naming `.with_handle(..)`, so a standalone server serving gateways over it
//! refuses to `listen()` before any I/O; one given a handle prepares.

use std::sync::Mutex;
use std::thread;

use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, StartupError, injectable, routes};
use ulo_ws::{BroadcastAdapter, WsModule};
use ulo_ws_redis::Redis;

/// Nothing listens here; `prepare` and `listen()` refuse before any connection is tried.
const URL: &str = "redis://127.0.0.1:6379";

#[test]
fn an_adapter_built_outside_a_runtime_and_given_none_refuses_in_prepare() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let refused = Redis::url(URL).prepare().expect_err("an adapter with no tokio runtime prepared");
    assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");

    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a tokio runtime for the adapter");
    Redis::url(URL).with_handle(runtime.handle().clone()).prepare().expect("an adapter given a handle prepares");
}

#[injectable]
struct Lone;

#[routes]
#[ulo_ws::gateway(path = "/lone", port = own)]
impl Lone {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// `WsModule` over the adapter it is given, and one gateway for the standalone server.
struct Root(Mutex<Option<Redis>>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(adapter) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(WsModule::for_root().broadcast(adapter));
        }
        m.controller::<Lone>();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_standalone_server_over_an_adapter_with_no_runtime_is_refused_at_listen() {
    let adapter = thread::spawn(|| Redis::url(URL)).join().expect("building the adapter on a plain thread panicked");
    let bound = App::builder(Root(Mutex::new(Some(adapter))))
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await;
    match bound {
        Err(StartupError::Configure(refused)) => {
            assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");
        }
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("handle")).await;
            panic!("a server over a broadcast adapter with no tokio runtime listened");
        }
    }
}
