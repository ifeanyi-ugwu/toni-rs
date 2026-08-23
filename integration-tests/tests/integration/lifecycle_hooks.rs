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
use toni::{
    injectable, module, on_application_bootstrap, on_module_init, toni_factory::ToniFactory,
};
use toni_axum::AxumAdapter;
use toni_macros::{
    controller, on_application_shutdown, on_module_destroy, patterns, routes, rpc_controller,
};
use toni_tcp::TcpAdapter;

static EVENT_LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();

fn get_log() -> Arc<Mutex<Vec<&'static str>>> {
    EVENT_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[injectable]
pub struct HookedService {}
impl HookedService {
    #[on_module_init]
    async fn on_module_init(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("provider:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn on_application_bootstrap(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("provider:bootstrap");
        Ok(())
    }
}

#[module(providers: [HookedService])]
impl HookModule {
    #[on_module_init]
    async fn on_module_init(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("module:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn on_module_bootstrap(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("module:bootstrap");
        Ok(())
    }
}

#[serial]
#[tokio_localset_test::localset_test]
async fn startup_hooks_fire_in_order() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(HookModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
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

// Module-impl hooks are collected by an attribute scan (provider hooks expand through
// the standalone macros, which resolve by path on their own). The scan must accept the
// path-qualified spelling too.
#[tokio_localset_test::localset_test]
async fn path_qualified_module_hook_attr_fires() {
    static LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();
    fn qualified_log() -> Arc<Mutex<Vec<&'static str>>> {
        LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new()))).clone()
    }

    #[module(providers: [])]
    impl QualifiedHookModule {
        #[toni::on_module_init]
        async fn on_module_init(&self) -> toni::InitResult {
            qualified_log().lock().unwrap().push("module:init");
            Ok(())
        }
    }

    let _app = ToniFactory::create(QualifiedHookModule).await;
    assert_eq!(qualified_log().lock().unwrap().clone(), vec!["module:init"]);
}

#[rpc_controller]
pub struct HookedRpcController {}

#[patterns]
impl HookedRpcController {
    #[on_module_init]
    async fn ready(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("rpc-controller:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn started(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("rpc-controller:bootstrap");
        Ok(())
    }
}

#[module(controllers: [HookedRpcController])]
impl RpcHookModule {}

/// An RPC controller is kept out of the module's provider map so nothing can inject it, and the
/// startup hooks still reach it. The map excluding it is the same one the hook loops read, so
/// dropping it there without a second home would have silenced these hooks and nothing else.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_rpc_controller_still_gets_its_startup_hooks() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(RpcHookModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    // The declared patterns need a transport to reach: `bind()` refuses an RPC controller with
    // no RPC adapter behind it.
    app.use_rpc_adapter(TcpAdapter::new("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["rpc-controller:init", "rpc-controller:bootstrap"],
        "an RPC controller's startup hooks must fire in the usual order"
    );
}

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

#[toni_macros::grpc_service(pub struct HookedGrpcService {})]
impl HookedGrpcService {
    #[toni_macros::new]
    pub fn new() -> Self {
        Self {}
    }

    #[on_module_init]
    async fn ready(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("grpc-service:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn started(&self) -> toni::InitResult {
        get_log().lock().unwrap().push("grpc-service:bootstrap");
        Ok(())
    }
}

#[toni_macros::grpc_methods]
#[tonic::async_trait]
impl orders_pb::orders_server::Orders for HookedGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        }))
    }

    type WatchProgressStream = std::pin::Pin<
        Box<
            dyn futures_util::Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send,
        >,
    >;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Ok(tonic::Response::new(
            Box::pin(futures_util::stream::empty()),
        ))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>,
    >;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Ok(tonic::Response::new(
            Box::pin(futures_util::stream::empty()),
        ))
    }
}

#[module(controllers: [HookedGrpcService])]
impl GrpcHookModule {}

/// A gRPC service is kept out of the module's provider map so nothing can inject it, and the
/// startup hooks still reach it. Its hooks are dispatched through the lifecycle bridge rather than
/// by name, because a service built per call has no `Provider` of its own to hang them on — this
/// fails if that rewiring drops them.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_grpc_service_still_gets_its_startup_hooks() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(GrpcHookModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["grpc-service:init", "grpc-service:bootstrap"],
        "a gRPC service's startup hooks must fire in the usual order"
    );
}

#[controller("/ctx")]
pub struct ContextHookedController {}

#[routes]
impl ContextHookedController {
    #[on_module_destroy]
    async fn torn_down(&self) {
        get_log().lock().unwrap().push("controller:destroy");
    }

    #[on_application_shutdown]
    async fn stopped(&self, _signal: Option<String>) {
        get_log().lock().unwrap().push("controller:shutdown");
    }
}

#[module(controllers: [ContextHookedController])]
impl ContextHookModule {}

/// An application context has no HTTP server, and its shutdown hooks used to reach providers only —
/// the controller pass lived on `ToniApplication`. Every dispatch target is a controller now, so a
/// worker built with `create_application_context` would otherwise close without running any of them.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_application_context_runs_its_controllers_shutdown_hooks() {
    get_log().lock().unwrap().clear();

    let mut ctx = ToniFactory::create_application_context(ContextHookModule).await;
    ctx.close().await;

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["controller:destroy", "controller:shutdown"],
        "a controller's teardown hooks must run when the context closes"
    );
}

/// The full application delegates its teardown to the context rather than running a controller pass
/// of its own. Each hook must therefore appear once, not twice — a second pass anywhere above the
/// context would show up here as a duplicate.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_application_runs_its_controllers_teardown_hooks_once() {
    get_log().lock().unwrap().clear();

    let mut app = ToniFactory::create(ContextHookModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();
    app.close().await;

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["controller:destroy", "controller:shutdown"],
        "a controller's teardown hooks must run exactly once when the application closes"
    );
}
