//! An adapter built on a thread with no tokio runtime current, and given none with `with_handle`,
//! fails its `prepare` naming `.with_handle(..)`, so an app importing `WsModule` over it fails
//! `connect` before any I/O, a gateway served or not; one given a handle prepares.

use std::sync::Mutex;
use std::thread;

use ulo::{App, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, StartupError};
use ulo_ws::{BroadcastAdapter, WsModule};
use ulo_ws_redis::Redis;

/// Nothing listens here; `prepare` and `connect` refuse before any connection is tried.
const URL: &str = "redis://127.0.0.1:6379";

#[test]
fn an_adapter_built_outside_a_runtime_and_given_none_refuses_in_prepare() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let refused = Redis::url(URL).prepare().expect_err("an adapter with no tokio runtime prepared");
    assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");

    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a tokio runtime for the adapter");
    Redis::url(URL).with_handle(runtime.handle().clone()).prepare().expect("an adapter given a handle prepares");
}

/// `WsModule` over the adapter it is given, serving no gateway: a process that only broadcasts.
struct Root(Mutex<Option<Redis>>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(adapter) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(WsModule::for_root().broadcast(adapter));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_broadcasting_over_an_adapter_with_no_runtime_fails_connect() {
    let adapter = thread::spawn(|| Redis::url(URL)).join().expect("building the adapter on a plain thread panicked");
    let wired = App::builder(Root(Mutex::new(Some(adapter)))).runtime(ulo_tokio::Tokio::current()).wire().expect("the app wires");
    match wired.connect().await {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) => {
            assert!(error.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {error}");
        }
        Err(other) => panic!("expected `WsModule`'s init hook to fail `connect`, got: {other}"),
        Ok(_) => panic!("an app connected over a broadcast adapter with no tokio runtime"),
    }
}
