// Startup lifecycle ordering is the contract this file exists to prove.
//
// The framework guarantees:
//   module:on_module_init → provider:on_module_init
//     → module:on_application_bootstrap → provider:on_application_bootstrap
//
// on_module_init fires during ToniFactory::create(); on_application_bootstrap
// fires during app.bind(). This split matters: providers that open connections
// in init are ready by the time bootstrap runs.

use std::sync::{Arc, Mutex, OnceLock};

use serial_test::serial;
use toni::{module, on_bootstrap, on_init, provider, toni_factory::ToniFactory};
use toni_axum::AxumAdapter;

static EVENT_LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();

fn get_log() -> Arc<Mutex<Vec<&'static str>>> {
    EVENT_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[provider]
pub struct HookedService {}
impl HookedService {
    #[on_init]
    async fn on_init(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("provider:init");
        Ok(())
    }

    #[on_bootstrap]
    async fn on_bootstrap(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("provider:bootstrap");
        Ok(())
    }
}

#[module(providers: [HookedService])]
impl HookModule {
    #[on_module_init]
    fn on_module_init(&self) {
        get_log().lock().unwrap().push("module:init");
    }

    #[on_application_bootstrap]
    fn on_module_bootstrap(&self) {
        get_log().lock().unwrap().push("module:bootstrap");
    }
}

#[serial]
#[tokio_localset_test::localset_test]
async fn startup_hooks_fire_in_order() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(HookModule::module_definition()).await;
    app.use_http_adapter(AxumAdapter::new(), 0, "127.0.0.1")
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            "module:init",
            "provider:init",
            "module:bootstrap",
            "provider:bootstrap",
        ],
        "expected module init → provider init → module bootstrap → provider bootstrap"
    );
}
