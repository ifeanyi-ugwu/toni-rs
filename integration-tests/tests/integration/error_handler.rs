// The framework's error model has two distinct paths:
//
//   1. User-handler errors render at the macro boundary via
//      `AppError::into_http_response()`. The error-handler chain does not
//      run on them — by the time anything else could see the response,
//      the user has already shaped it through their own `AppError` impl.
//
//   2. Framework-generated errors (guard rejection, missing route,
//      middleware failure) run through the chain. The chain dispatches on
//      the framework's typed event (today: `HttpError` synthesised from
//      response status — semantic event types come in a follow-up).
//
// Tests below exercise both paths.

use std::sync::Arc;

use toni::{
    Body as ToniBody, HttpResponse, async_trait, context::HttpContext, controller,
    errors::{GuardRejection, HttpError},
    get, module, toni_factory::ToniFactory,
    traits_helpers::{ChainError, ErrorHandler, Guard},
};
use toni_axum::AxumAdapter;
use toni_macros::use_guards;

// ---- AppError-rendered responses (no chain involvement) ---------------------

#[tokio_localset_test::localset_test]
async fn http_error_renders_via_app_error_default() {
    #[controller("/api", pub struct HttpErrController {})]
    impl HttpErrController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("resource not found"))
        }
    }

    #[module(controllers: [HttpErrController], providers: [])]
    impl HttpErrModule {}

    let addr = start_app(HttpErrModule::module_definition(), None).await;

    let resp = reqwest::get(format!("http://{}/api/missing", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 404);
    assert_eq!(body["error"], "Not Found");
    assert_eq!(body["message"], "resource not found");
}

#[derive(Debug, toni::AppError)]
#[app_error(NotFound)]
struct InvoiceMissing(String);

impl std::fmt::Display for InvoiceMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invoice {} not found", self.0)
    }
}

impl std::error::Error for InvoiceMissing {}

#[tokio_localset_test::localset_test]
async fn custom_app_error_renders_canonical_envelope() {
    #[controller("/api", pub struct CustomErrController {})]
    impl CustomErrController {
        #[get("/invoice")]
        fn invoice(&self) -> Result<ToniBody, InvoiceMissing> {
            Err(InvoiceMissing("inv-42".into()))
        }
    }

    #[module(controllers: [CustomErrController], providers: [])]
    impl CustomErrModule {}

    let addr = start_app(CustomErrModule::module_definition(), None).await;

    let resp = reqwest::get(format!("http://{}/api/invoice", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 404);
    assert_eq!(body["error"], "Not Found");
    // Default `message()` uses Display.
    assert_eq!(body["message"], "invoice inv-42 not found");
}

#[tokio_localset_test::localset_test]
async fn chain_does_not_fire_on_user_error() {
    // Sanity: a registered global ErrorHandler<HttpContext, HttpResponse> must
    // *not* intercept user errors. If it did, the user's AppError rendering
    // wouldn't be the source of truth.
    #[controller("/api", pub struct UserErrController {})]
    impl UserErrController {
        #[get("/bad")]
        fn bad(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::bad_request("user-error"))
        }
    }

    #[module(controllers: [UserErrController], providers: [])]
    impl UserErrModule {}

    let addr = start_app(
        UserErrModule::module_definition(),
        Some(Arc::new(MarkerHandler { marker: "REWRITTEN" })),
    )
    .await;

    let resp = reqwest::get(format!("http://{}/api/bad", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Canonical envelope, not the chain's `MarkerHandler` rewrite.
    assert_eq!(body["statusCode"], 400);
    assert_eq!(body["message"], "user-error");
    assert_ne!(body["message"], "REWRITTEN");
}

// ---- Chain on framework-generated errors (guard rejection) ------------------

struct AlwaysReject;

#[async_trait]
impl Guard<HttpContext> for AlwaysReject {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        false
    }
}

struct MarkerHandler {
    marker: &'static str,
}

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for MarkerHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        // Chain handlers downcast to the framework's typed event — there's
        // no synthesized `HttpError` to dispatch on anymore.
        error.downcast_ref::<GuardRejection>()?;
        let mut resp = HttpResponse::new();
        resp.status = 403;
        resp.body = Some(ToniBody::text(self.marker));
        Some(resp)
    }
}

#[tokio_localset_test::localset_test]
async fn chain_fires_on_guard_rejection() {
    #[controller("/api", pub struct GuardedController {})]
    impl GuardedController {
        #[get("/protected")]
        #[use_guards(AlwaysReject {})]
        fn protected(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("should not reach"))
        }
    }

    #[module(controllers: [GuardedController], providers: [])]
    impl GuardedModule {}

    let addr = start_app(
        GuardedModule::module_definition(),
        Some(Arc::new(MarkerHandler {
            marker: "guard-caught",
        })),
    )
    .await;

    let resp = reqwest::get(format!("http://{}/api/protected", addr))
        .await
        .unwrap();
    // Guard rejection produces 403; chain handler claims it and rewrites the body.
    assert_eq!(resp.status(), 403);
    assert_eq!(resp.text().await.unwrap(), "guard-caught");
}

// ---- Test harness -----------------------------------------------------------

async fn start_app(
    module: toni::module_helpers::module_enum::ModuleDefinition,
    chain_handler: Option<Arc<dyn ErrorHandler<HttpContext, HttpResponse>>>,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        if let Some(handler) = chain_handler {
            factory.use_global_http_error_handler(handler);
        }
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
