//! `provider_factory!` and `provider_value!` register enhancer roles by detecting the trait impls
//! on the produced value's type — no `guard`/`interceptor` argument.
//!
//! - singleton (default): the built value is probed directly in `build()`;
//! - request-scoped: the closure's written `-> T` names the type, gating the per-request factory.
//!
//! Each guard below is applied with `#[use_guards("TOKEN")]` and gates a route 403/200.

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::{
    Body as ToniBody, controller, routes, get, module, provider_factory, provider_value, use_guards,
};

use crate::common::TestServer;
use serial_test::serial;

// A guard registered as a singleton value under a string token — detected from the type, no marker.
#[derive(Clone)]
pub struct ValueGuard;

#[async_trait]
impl Guard<HttpContext> for ValueGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-value")
    }
}

// A guard built by a factory closure; request-scoped, so the closure's `-> FactoryGuard` names the
// type for the registration gate.
#[derive(Clone)]
pub struct FactoryGuard;

#[async_trait]
impl Guard<HttpContext> for FactoryGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-factory")
    }
}

#[controller("/pf")]
pub struct GuardedController;

#[routes]
impl GuardedController {
    #[get("/value")]
    #[use_guards("VALUE_GUARD")]
    fn value_route(&self) -> ToniBody {
        ToniBody::text("value ok".to_string())
    }

    #[get("/factory")]
    #[use_guards("FACTORY_GUARD")]
    fn factory_route(&self) -> ToniBody {
        ToniBody::text("factory ok".to_string())
    }
}

#[module(
    controllers: [GuardedController],
    providers: [
        // value provider + type hint → stored concretely → role auto-detected. No `guard` arg.
        provider_value!("VALUE_GUARD", ValueGuard, ValueGuard),
        // request-scoped factory; `-> FactoryGuard` names the produced type for the gate.
        provider_factory!("FACTORY_GUARD", || -> FactoryGuard { FactoryGuard }, scope = "request"),
    ],
)]
struct ProviderMacroModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn provider_value_guard_autodetected() {
    let server = TestServer::start(ProviderMacroModule::module_definition()).await;

    let resp = server
        .client()
        .get(server.url("/pf/value"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "value guard blocks without x-value");

    let resp = server
        .client()
        .get(server.url("/pf/value"))
        .header("X-Value", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "value ok");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn provider_factory_request_scoped_guard_autodetected() {
    let server = TestServer::start(ProviderMacroModule::module_definition()).await;

    let resp = server
        .client()
        .get(server.url("/pf/factory"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "factory guard blocks without x-factory");

    let resp = server
        .client()
        .get(server.url("/pf/factory"))
        .header("X-Factory", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "factory ok");
}
