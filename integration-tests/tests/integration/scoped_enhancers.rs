//! Integration tests for request- and transient-scoped providers acting as
//! guards and interceptors. These verify that scope does not prevent a provider
//! from contributing to the enhancer pipeline — a fresh instance is constructed
//! per request using the DynGuardFactory / DynInterceptorFactory path.

use serial_test::serial;
use toni::async_trait;
use toni::{guard, interceptor};
use toni::injector::Context;
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};
use toni::{
    controller, get, injectable, module, use_guards, use_interceptors, Body as ToniBody, Request,
};

use crate::common::TestServer;

// ---- request-scoped guard, no injected deps ----------------------------------

#[injectable(scope = "request", pub struct RequestGuard {})]
#[guard]
impl RequestGuard {}

impl Guard for RequestGuard {
    fn can_activate(&self, context: &Context) -> bool {
        context
            .switch_to_http()
            .expect("HTTP context required")
            .request()
            .headers
            .contains_key("x-allow")
    }
}

// ---- request-scoped guard that injects Request -------------------------------

#[injectable(scope = "request", pub struct HeaderGuard {
    #[inject]
    request: Request,
})]
#[guard]
impl HeaderGuard {}

impl Guard for HeaderGuard {
    fn can_activate(&self, _context: &Context) -> bool {
        self.request
            .header("x-secret")
            .map_or(false, |v| v == "open-sesame")
    }
}

// ---- transient-scoped interceptor --------------------------------------------

#[injectable(scope = "transient", pub struct TransientInterceptor {})]
#[interceptor]
impl TransientInterceptor {}

#[async_trait]
impl Interceptor for TransientInterceptor {
    async fn intercept(&self, context: &mut Context, next: Box<dyn InterceptorNext>) {
        next.run(context).await;
        context
            .switch_to_http_mut()
            .expect("HTTP context required")
            .response_mut()
            .unwrap()
            .headers
            .push(("x-transient".to_string(), "hit".to_string()));
    }
}

// ---- controllers -------------------------------------------------------------

#[controller("/gate", pub struct GateController {})]
#[use_guards(RequestGuard)]
impl GateController {
    #[get("/check")]
    fn check(&self) -> ToniBody {
        ToniBody::text("passed".to_string())
    }
}

#[controller("/secret", pub struct SecretController {})]
#[use_guards(HeaderGuard)]
impl SecretController {
    #[get("/unlock")]
    fn unlock(&self) -> ToniBody {
        ToniBody::text("unlocked".to_string())
    }
}

#[controller("/transient", pub struct TransientController {})]
#[use_interceptors(TransientInterceptor)]
impl TransientController {
    #[get("/ping")]
    fn ping(&self) -> ToniBody {
        ToniBody::text("pong".to_string())
    }
}

// ---- modules -----------------------------------------------------------------

#[module(
    controllers: [GateController],
    providers: [RequestGuard],
)]
impl RequestGuardModule {}

#[module(
    controllers: [SecretController],
    providers: [HeaderGuard],
)]
impl HeaderGuardModule {}

#[module(
    controllers: [TransientController],
    providers: [TransientInterceptor],
)]
impl TransientInterceptorModule {}

// ---- tests -------------------------------------------------------------------

#[serial]
#[tokio_localset_test::localset_test]
async fn request_scoped_guard_activates() {
    let server = TestServer::start(RequestGuardModule::module_definition()).await;

    // Missing header — guard blocks
    let resp = server
        .client()
        .get(server.url("/gate/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Header present — guard passes
    let resp = server
        .client()
        .get(server.url("/gate/check"))
        .header("x-allow", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "passed");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn request_scoped_guard_injects_request() {
    let server = TestServer::start(HeaderGuardModule::module_definition()).await;

    // Wrong secret — rejected
    let resp = server
        .client()
        .get(server.url("/secret/unlock"))
        .header("x-secret", "wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Correct secret — allowed
    let resp = server
        .client()
        .get(server.url("/secret/unlock"))
        .header("x-secret", "open-sesame")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "unlocked");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn transient_scoped_interceptor() {
    let server = TestServer::start(TransientInterceptorModule::module_definition()).await;

    let resp = server
        .client()
        .get(server.url("/transient/ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-transient").unwrap(), "hit");
    assert_eq!(resp.text().await.unwrap(), "pong");
}
