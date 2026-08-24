//! Lifecycle hooks on `#[injectable]` structs via the `#[on_*]` bridge.
//!
//! The macro can't see the impl, so it dispatches every `Provider` lifecycle method through an
//! inherent bridge fn the `#[on_module_init]` / `#[on_application_bootstrap]` / `#[on_module_destroy]` macros emit. A provider
//! with no hooks gets the blanket no-op; one with hooks runs them. Mirrors `lifecycle_hooks.rs`
//! (the older attribute form), but every provider here is a plain `#[injectable]` struct.

use std::sync::{Arc, Mutex, OnceLock};

use serial_test::serial;
use toni::{
    before_application_shutdown, injectable, module, on_application_bootstrap,
    on_application_shutdown, on_module_destroy, on_module_init, toni_factory::ToniFactory,
};
use toni_axum::AxumAdapter;

static EVENT_LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();

fn get_log() -> Arc<Mutex<Vec<&'static str>>> {
    EVENT_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[injectable]
pub struct HookedService {
    #[default(0)]
    _marker: u8,
}

impl HookedService {
    #[on_module_init]
    async fn init(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn bootstrap(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("bootstrap");
        Ok(())
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        get_log().lock().unwrap().push("destroy");
    }

    #[before_application_shutdown]
    async fn before_application_shutdown(&self, _signal: Option<String>) {
        get_log()
            .lock()
            .unwrap()
            .push("before_application_shutdown");
    }

    #[on_application_shutdown]
    async fn shutdown(&self, _signal: Option<String>) {
        get_log().lock().unwrap().push("shutdown");
    }
}

// A derived provider with NO lifecycle hooks — must build and run fine (blanket no-op bridge).
#[injectable]
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

    let mut app = ToniFactory::create(LifecycleModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["init", "bootstrap"],
        "derive provider's #[on_module_init] then #[on_application_bootstrap] must fire during create()/bind()"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn derive_shutdown_hooks_fire() {
    get_log().lock().unwrap().clear();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(LifecycleModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
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
    // before_application_shutdown → destroy → shutdown).
    assert!(
        log.contains(&"before_application_shutdown"),
        "before_application_shutdown must fire on shutdown; got {:?}",
        log
    );
    assert!(
        log.contains(&"destroy"),
        "on_module_destroy must fire on shutdown; got {:?}",
        log
    );
    assert!(
        log.contains(&"shutdown"),
        "on_application_shutdown must fire on shutdown; got {:?}",
        log
    );
}
