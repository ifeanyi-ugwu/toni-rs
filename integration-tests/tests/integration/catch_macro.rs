//! `#[catch(T)]` — function-style ErrorHandler escape hatch.
//!
//! Verifies the macro lowers a free async fn into a unit struct whose
//! `ErrorHandler<C, R>` impl downcasts to `T` and short-circuits with
//! `None` for non-matching types so the chain advances.
//!
//! Because the chain only runs on framework-generated errors (per the
//! error redesign), these tests exercise `#[catch]` against
//! guard-rejection responses — that's where the chain still fires.

use std::sync::Arc;

use toni::{
    async_trait, catch,
    context::HttpContext,
    controller,
    errors::{GuardRejection, HttpError},
    get, module, routes,
    toni_factory::ToniFactory,
    traits_helpers::Guard,
    Body as ToniBody, Error, HttpResponse,
};
use toni_axum::AxumAdapter;
use toni_macros::use_guards;

#[catch(GuardRejection)]
async fn guard_catcher(err: &GuardRejection, _ctx: &HttpContext) -> HttpResponse {
    let mut resp = HttpResponse::new();
    resp.status = toni::errors::http_status(err.kind());
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
    assert_impls::<guard_catcher>();
    assert_impls::<other_catcher>();
}

// Guard that always rejects — produces the framework-generated 403 the
// chain will fire on.
struct DenyGuard;

#[async_trait]
impl Guard<HttpContext> for DenyGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        false
    }
}

async fn start_with_catchers(module: impl toni::ModuleMetadata + 'static) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        // Both registered. `other_catcher` is consulted first (later
        // registration → higher priority via reverse iteration). It must
        // return None because the boxed event is `GuardRejection`, not
        // `OtherError`; then `guard_catcher` claims it.
        factory.use_global_http_error_handler(Arc::new(guard_catcher));
        factory.use_global_http_error_handler(Arc::new(other_catcher));
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

#[tokio_localset_test::localset_test]
async fn catch_handler_intercepts_framework_error() {
    // Guard rejection produces a framework-generated 403 → chain runs →
    // http_catcher claims it.
    #[controller("/api")]
    pub struct CatchTestController {}

    #[routes]
    impl CatchTestController {
        #[get("/protected")]
        #[use_guards(DenyGuard {})]
        fn protected(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("should not reach"))
        }
    }

    #[module(controllers: [CatchTestController], providers: [])]
    impl CatchTestModule {}

    let addr = start_with_catchers(CatchTestModule).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/protected", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap();
    assert!(
        body.starts_with("catch:"),
        "expected catch envelope, got: {body}"
    );
}

#[tokio_localset_test::localset_test]
async fn non_matching_catch_falls_through() {
    // Sanity: a catcher whose target type doesn't match the boxed error must
    // return None so the chain advances. If our downcast were buggy and
    // always matched, this would render "OTHER-CAUGHT" instead of catching.
    #[controller("/api")]
    pub struct FallthroughController {}

    #[routes]
    impl FallthroughController {
        #[get("/protected")]
        #[use_guards(DenyGuard {})]
        fn protected(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("should not reach"))
        }
    }

    #[module(controllers: [FallthroughController], providers: [])]
    impl FallthroughModule {}

    let addr = start_with_catchers(FallthroughModule).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/protected", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap();
    // http_catcher matched; other_catcher returned None and the chain
    // advanced past it.
    assert!(
        body.starts_with("catch:"),
        "expected http_catcher to claim, got: {body}"
    );
    assert_ne!(body, "OTHER-CAUGHT");
}
