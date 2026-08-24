// The framework's error model has two distinct paths:
//
//   1. User-handler errors render at the macro boundary via
//      `HttpError::to_response()`. The error-handler chain does not
//      run on them — by the time anything else could see the response,
//      the user has already shaped it through their own `toni::Error` impl.
//
//   2. Framework-generated errors (guard rejection, missing route,
//      middleware failure) run through the chain. The chain dispatches on
//      the framework's typed event (today: `HttpError` synthesised from
//      response status — semantic event types come in a follow-up).
//
// Tests below exercise both paths.

use std::sync::Arc;

use toni::{
    async_trait,
    context::HttpContext,
    controller,
    errors::{GuardRejection, HttpError},
    get, module, routes,
    toni_factory::ToniFactory,
    traits_helpers::{ChainError, ErrorHandler, Guard},
    Body as ToniBody, HttpResponse,
};
use toni_axum::AxumAdapter;
use toni_macros::use_guards;

// ---- Canonical-envelope responses (no chain involvement) ---------------------

#[tokio_localset_test::localset_test]
async fn http_error_renders_via_app_error_default() {
    #[controller("/api")]
    pub struct HttpErrController {}

    #[routes]
    impl HttpErrController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("resource not found"))
        }
    }

    #[module(controllers: [HttpErrController], providers: [])]
    impl HttpErrModule {}

    let addr = start_app(HttpErrModule, None).await;

    let resp = reqwest::get(format!("http://{}/api/missing", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 404);
    assert_eq!(body["error"], "Not Found");
    assert_eq!(body["message"], "resource not found");
}

#[derive(Debug, toni::Error)]
#[error_kind(NotFound)]
struct InvoiceMissing(String);

impl std::fmt::Display for InvoiceMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invoice {} not found", self.0)
    }
}

impl std::error::Error for InvoiceMissing {}

#[tokio_localset_test::localset_test]
async fn custom_app_error_renders_canonical_envelope() {
    #[controller("/api")]
    pub struct CustomErrController {}

    #[routes]
    impl CustomErrController {
        #[get("/invoice")]
        fn invoice(&self) -> Result<ToniBody, InvoiceMissing> {
            Err(InvoiceMissing("inv-42".into()))
        }
    }

    #[module(controllers: [CustomErrController], providers: [])]
    impl CustomErrModule {}

    let addr = start_app(CustomErrModule, None).await;

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
async fn unmatched_chain_handler_falls_through_to_app_error_default() {
    // The chain runs on user errors (symmetric with framework events), but
    // a handler that downcasts to a *different* type than the boxed user
    // error returns None and the chain advances. With no claim, the
    // canonical envelope is the response.
    #[controller("/api")]
    pub struct UserErrController {}

    #[routes]
    impl UserErrController {
        #[get("/bad")]
        fn bad(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::bad_request("user-error"))
        }
    }

    #[module(controllers: [UserErrController], providers: [])]
    impl UserErrModule {}

    // MarkerHandler downcasts to `GuardRejection` — it must NOT claim a
    // user `HttpError`. The boxed error is HttpError, downcast returns None,
    // chain falls through, the canonical envelope renders the canonical envelope.
    let addr = start_app(
        UserErrModule,
        Some(Arc::new(MarkerHandler {
            marker: "REWRITTEN",
        })),
    )
    .await;

    let resp = reqwest::get(format!("http://{}/api/bad", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
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
    #[controller("/api")]
    pub struct GuardedController {}

    #[routes]
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
        GuardedModule,
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

// ---- Per-scope override on a user error type --------------------------------

struct HttpErrorOverride;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for HttpErrorOverride {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        let e = error.downcast_ref::<HttpError>()?;
        let mut resp = HttpResponse::new();
        resp.status = e.status_code();
        resp.body = Some(ToniBody::text(format!("scope-override:{}", e.message())));
        Some(resp)
    }
}

#[tokio_localset_test::localset_test]
async fn scope_chain_overrides_app_error_default_on_user_error() {
    // Stratification in action: `toni::Error` is the type-level default, the
    // chain is the scope-level override. A handler registered on this scope
    // that downcasts to the user error type wins; everywhere else, the
    // type's canonical envelope is the response.
    #[controller("/api")]
    pub struct UserErrController {}

    #[routes]
    impl UserErrController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("user-error"))
        }
    }

    #[module(controllers: [UserErrController], providers: [])]
    impl UserErrModule {}

    let addr = start_app(UserErrModule, Some(Arc::new(HttpErrorOverride))).await;

    let resp = reqwest::get(format!("http://{}/api/missing", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    // Chain handler claimed the user error and replaced the canonical
    // envelope with the scope-specific shape.
    assert_eq!(body, "scope-override:user-error");
}

// ---- Test harness -----------------------------------------------------------

async fn start_app(
    module: impl toni::ModuleMetadata + 'static,
    chain_handler: Option<Arc<dyn ErrorHandler<HttpContext, HttpResponse>>>,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        if let Some(handler) = chain_handler {
            factory.use_global_http_error_handler(handler);
        }
        let mut app = factory.create_with(module).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
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
