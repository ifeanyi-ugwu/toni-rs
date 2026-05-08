// Error handlers are the framework's last line of defense: a controller returning
// Err(HttpError) must reach a registered handler and map to the correct HTTP status.
//
// Two contracts tested here:
//   1. A global handler (registered on ToniFactory) intercepts HttpError and converts
//      it to the right status + body — verifying the default error propagation path.
//   2. A method-level handler runs before the global fallback (chain of responsibility):
//      the method handler owns 400s; everything else falls through to global.

use std::sync::Arc;

use toni::{
    async_trait, context::HttpContext, controller,
    errors::HttpError,
    get, module,
    toni_factory::ToniFactory,
    traits_helpers::ErrorHandler,
    Body as ToniBody, HttpResponse,
};
use toni_axum::AxumAdapter;
use toni_macros::use_error_handlers;

struct GlobalHandler;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for GlobalHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        if let Some(e) = error.downcast_ref::<HttpError>() {
            let mut resp = HttpResponse::new();
            resp.status = e.status_code();
            resp.body = Some(ToniBody::text(format!("global:{}", e.message())));
            return Some(resp);
        }
        None
    }
}

struct BadRequestHandler;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for BadRequestHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        if let Some(e) = error.downcast_ref::<HttpError>() {
            if e.status_code() == 400 {
                let mut resp = HttpResponse::new();
                resp.status = 400;
                resp.body = Some(ToniBody::text(format!("method:{}", e.message())));
                return Some(resp);
            }
        }
        None
    }
}

async fn start_with_global_handler(
    module: toni::module_helpers::module_enum::ModuleDefinition,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        factory.use_global_http_error_handler(Arc::new(GlobalHandler));
        let mut app = factory.create_with(module).await;
        app.use_http_adapter(AxumAdapter::new(), 0, "127.0.0.1")
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.http.expect("HTTP adapter not bound"));
        app.run().await;
    });

    tokio::task::spawn_local(async move {
        local.await;
    });

    addr_rx.await.unwrap()
}

#[tokio_localset_test::localset_test]
async fn global_error_handler_intercepts_http_error() {
    #[controller("/api", pub struct TestController {})]
    impl TestController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("resource not found"))
        }
    }

    #[module(controllers: [TestController], providers: [])]
    impl TestModule {}

    let addr = start_with_global_handler(TestModule::module_definition()).await;

    let resp = reqwest::get(format!("http://{}/api/missing", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.text().await.unwrap(), "global:resource not found");
}

#[tokio_localset_test::localset_test]
async fn method_error_handler_runs_before_global() {
    #[controller("/api", pub struct TestController {})]
    impl TestController {
        #[get("/bad")]
        #[use_error_handlers(BadRequestHandler {})]
        fn bad(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::bad_request("invalid input"))
        }

        #[get("/gone")]
        #[use_error_handlers(BadRequestHandler {})]
        fn gone(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("not found"))
        }
    }

    #[module(controllers: [TestController], providers: [])]
    impl TestModule {}

    let addr = start_with_global_handler(TestModule::module_definition()).await;

    let client = reqwest::Client::new();

    // 400: method handler claims it, global never runs
    let resp = client
        .get(format!("http://{}/api/bad", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.text().await.unwrap(), "method:invalid input");

    // 404: method handler returns None (only handles 400), falls through to global
    let resp = client
        .get(format!("http://{}/api/gone", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.text().await.unwrap(), "global:not found");
}
