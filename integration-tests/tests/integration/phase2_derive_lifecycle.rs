//! Lifecycle hooks on `#[derive(Injectable)]` providers via the `#[on_*]` bridge.
//!
//! The derive can't see the impl, so it dispatches every `Provider` lifecycle method through an
//! inherent bridge fn the `#[on_init]` / `#[on_bootstrap]` / `#[on_destroy]` macros emit. A provider
//! with no hooks gets the blanket no-op; one with hooks runs them. Mirrors `lifecycle_hooks.rs`
//! (the attribute-form equivalent), but every provider here is a plain derived struct.

use std::sync::{Arc, Mutex, OnceLock};

use serial_test::serial;
use toni::{
    Injectable, before_shutdown, module, on_bootstrap, on_destroy, on_init, on_shutdown,
    toni_factory::ToniFactory,
};
use toni_axum::AxumAdapter;

static EVENT_LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();

fn get_log() -> Arc<Mutex<Vec<&'static str>>> {
    EVENT_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[derive(Clone, Injectable)]
pub struct HookedService {
    #[default(0)]
    _marker: u8,
}

impl HookedService {
    #[on_init]
    async fn init(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("init");
        Ok(())
    }

    #[on_bootstrap]
    async fn bootstrap(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("bootstrap");
        Ok(())
    }

    #[on_destroy]
    async fn destroy(&self) {
        get_log().lock().unwrap().push("destroy");
    }

    #[before_shutdown]
    async fn before_shutdown(&self, _signal: Option<String>) {
        get_log().lock().unwrap().push("before_shutdown");
    }

    #[on_shutdown]
    async fn shutdown(&self, _signal: Option<String>) {
        get_log().lock().unwrap().push("shutdown");
    }
}

// A derived provider with NO lifecycle hooks — must build and run fine (blanket no-op bridge).
#[derive(Clone, Injectable)]
pub struct PlainService {
    #[default(0)]
    _marker: u8,
}

#[module(providers: [HookedService, PlainService])]
struct LifecycleModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn derive_startup_hooks_fire() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(LifecycleModule::module_definition()).await;
    app.use_http_adapter(AxumAdapter::new(), 0, "127.0.0.1")
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["init", "bootstrap"],
        "derive provider's #[on_init] then #[on_bootstrap] must fire during create()/bind()"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn derive_shutdown_hooks_fire() {
    get_log().lock().unwrap().clear();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(LifecycleModule::module_definition()).await;
        app.use_http_adapter(AxumAdapter::new(), 0, "127.0.0.1")
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let shutdown = shutdown_rx.await.unwrap();
    shutdown.shutdown();
    shutdown.completed().await;

    let log = get_log().lock().unwrap().clone();
    // The shutdown sequence (the framework's documented teardown order is
    // before_shutdown → destroy → shutdown).
    assert!(
        log.contains(&"before_shutdown"),
        "before_shutdown must fire on shutdown; got {:?}",
        log
    );
    assert!(log.contains(&"destroy"), "on_destroy must fire on shutdown; got {:?}", log);
    assert!(log.contains(&"shutdown"), "on_shutdown must fire on shutdown; got {:?}", log);
}
