//! Lifecycle hooks on an `#[injectable]` struct: `#[on_module_init]` and its
//! four siblings attach to an inherent impl, and the framework calls them.
//!
//! `#[injectable]` never sees that impl block, so each `#[on_*]` macro emits an
//! inherent bridge fn the generated `Provider` dispatches through, and a
//! provider writing no hooks resolves the blanket no-op. "The hook exists" and
//! "the hook ran" are therefore separate claims; all five hooks assert the
//! second.

use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock};

use serial_test::serial;
use ulo::{
    UloFactory, before_application_shutdown, injectable, module, on_application_bootstrap,
    on_application_shutdown, on_module_destroy, on_module_init,
};
use ulo_http_axum::AxumAdapter;

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
    async fn init(&self) -> ulo::di::InitResult {
        get_log().lock().unwrap().push("init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn bootstrap(&self) -> ulo::di::InitResult {
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

/// A hook-free transient that is not `Sync` compiles: the refusal of a hook on a non-singleton
/// scope reads a flag on every such type, and a `Cell` field makes this one `!Sync`.
#[injectable(scope = "transient")]
pub struct Draft {
    #[default(Cell::new(3))]
    count: Cell<u32>,
}

#[module(providers: [Draft])]
struct DraftModule {}

#[tokio::test]
async fn a_hook_free_transient_that_is_not_sync_builds() {
    let app = UloFactory::create(DraftModule).await.unwrap();
    let draft = app.get::<Draft>().await.expect("Draft should resolve");
    assert_eq!(draft.count.get(), 3);
}

#[serial]
#[tokio::test]
async fn derive_startup_hooks_fire() {
    get_log().lock().unwrap().clear();

    let mut app = UloFactory::create(LifecycleModule).await.unwrap();
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
#[tokio::test]
async fn derive_shutdown_hooks_fire() {
    get_log().lock().unwrap().clear();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();

    tokio::spawn(async move {
        let mut app = UloFactory::create(LifecycleModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });

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
