//! `#[catch(T)]` — function-style ErrorHandler escape hatch for cases
//! `AppError` doesn't reach (typically re-shaping framework-synthesised
//! errors per route/controller).
//!
//! Verifies the macro lowers a free async fn into a unit struct whose
//! `ErrorHandler<C, R>` impl downcasts to `T` and short-circuits with
//! `None` for non-matching types so the chain advances.

use std::sync::Arc;

use toni::{
    Body as ToniBody, HttpResponse, async_trait, catch, context::HttpContext, controller,
    errors::HttpError, get, module, toni_factory::ToniFactory,
};
use toni_axum::AxumAdapter;
use toni_macros::use_error_handlers;

#[catch(HttpError)]
async fn http_catcher(err: &HttpError, _ctx: &HttpContext) -> HttpResponse {
    let mut resp = HttpResponse::new();
    resp.status = err.status_code();
    resp.body = Some(ToniBody::text(format!("catch:{}", err.message())));
    resp
}

// A non-HttpError type — used only to verify downcast_ref returns None and
// the chain falls through.
#[derive(Debug)]
struct OtherError;

impl std::fmt::Display for OtherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("other")
    }
}

impl std::error::Error for OtherError {}

#[catch(OtherError)]
async fn other_catcher(_err: &OtherError, _ctx: &HttpContext) -> HttpResponse {
    let mut resp = HttpResponse::new();
    resp.status = 500;
    resp.body = Some(ToniBody::text("OTHER-CAUGHT"));
    resp
}

// Static type-level checks: the macro must produce real `ErrorHandler` impls
// rather than something that merely compiles as a value.
#[test]
fn catch_struct_implements_error_handler_trait() {
    fn assert_impls<T: toni::traits_helpers::ErrorHandler<HttpContext, HttpResponse>>() {}
    assert_impls::<http_catcher>();
    assert_impls::<other_catcher>();
}

async fn start_with_catcher(
    module: toni::module_helpers::module_enum::ModuleDefinition,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        // Register both — `other_catcher` is consulted first (later registration =
        // higher priority via reverse iteration in the dispatcher) and must
        // fall through because the boxed error is HttpError, not OtherError.
        factory.use_global_http_error_handler(Arc::new(http_catcher));
        factory.use_global_http_error_handler(Arc::new(other_catcher));
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
async fn catch_handler_intercepts_matching_type() {
    #[controller("/api", pub struct CatchTestController {})]
    impl CatchTestController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("gone"))
        }
    }

    #[module(controllers: [CatchTestController], providers: [])]
    impl CatchTestModule {}

    let addr = start_with_catcher(CatchTestModule::module_definition()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/missing", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "catch:gone");
}

#[tokio_localset_test::localset_test]
async fn non_matching_catch_falls_through() {
    // Sanity: a catcher whose target type doesn't match the boxed error must
    // return None so the chain advances. If our downcast were buggy and always
    // matched, this would render OTHER-CAUGHT instead.
    #[controller("/api", pub struct FallthroughController {})]
    impl FallthroughController {
        #[get("/conflict")]
        fn conflict(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::conflict("dup"))
        }
    }

    #[module(controllers: [FallthroughController], providers: [])]
    impl FallthroughModule {}

    let addr = start_with_catcher(FallthroughModule::module_definition()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/conflict", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 409);
    let body = resp.text().await.unwrap();
    // http_catcher is the one that matched; other_catcher returned None.
    assert_eq!(body, "catch:dup");
}
