//! Phase 2 foundation: a guard with no `#[guard]` marker, resolved from DI by its concrete
//! type and coerced to `Arc<dyn Guard<HttpContext>>` at a type-known site.
//!
//! This is the mechanism `#[use_guards(X)]` will generate once the controller macro threads
//! it through (the remaining plumbing). It proves the three load-bearing claims independently
//! of that plumbing:
//!   1. a `#[provider]` guard with its own injected dependency resolves by type;
//!   2. `Arc<Guard> as Arc<dyn Guard<HttpContext>>` compiles — i.e. the coercion is valid and a
//!      missing `Guard` impl would be a loud compile error at this site, not a runtime registry miss;
//!   3. the coerced trait object runs against a real `HttpContext`.
//!
//! Contrast `enhancers_di.rs`, where the equivalent `AdminGuard` needs `#[injectable(struct …)]`
//! plus a `#[guard(http)]` marker to land in the role registry.

use std::sync::Arc;

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::{module, provider, toni_factory::ToniFactory};

// A plain dependency — field injection, no struct-in-macro.
#[provider]
pub struct AuthPolicy {
    #[default(true)]
    enabled: bool,
}

impl AuthPolicy {
    fn permits(&self, ctx: &HttpContext) -> bool {
        self.enabled && ctx.request().headers.contains_key("x-admin-token")
    }
}

// The guard: `#[provider]` + a normal `impl Guard<HttpContext>`. No `#[guard]` marker,
// no struct-in-macro, no transport restated. The trait impl is the whole declaration.
#[provider]
pub struct AdminGuard {
    #[inject]
    policy: AuthPolicy,
}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        self.policy.permits(ctx)
    }
}

#[module(providers: [AuthPolicy, AdminGuard])]
struct GuardModule {}

fn ctx_with_admin_header() -> HttpContext {
    let parts = http::Request::builder()
        .header("x-admin-token", "valid")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    HttpContext::from_parts(parts)
}

fn ctx_without_admin_header() -> HttpContext {
    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    HttpContext::from_parts(parts)
}

#[tokio_localset_test::localset_test]
async fn marker_free_guard_resolves_coerces_and_runs() {
    let app = ToniFactory::create_application_context(GuardModule::module_definition()).await;

    // (1) Resolution by concrete type. AdminGuard is built with AuthPolicy injected, despite
    //     carrying no #[guard] marker — it is just a normal provider.
    let guard = app
        .get::<AdminGuard>()
        .await
        .expect("AdminGuard resolves from DI by its concrete type");

    // (2) Use-site coercion. This line compiles only because AdminGuard: Guard<HttpContext>;
    //     drop the trait impl and it fails to compile here — the loud, local error phase 2 wants.
    let dyn_guard: Arc<dyn Guard<HttpContext>> = Arc::new(guard);

    // (3) Invoke through the trait object against a real context, exercising the injected dep.
    assert!(
        dyn_guard.can_activate(&ctx_with_admin_header()).await,
        "guard should admit a request carrying x-admin-token"
    );
    assert!(
        !dyn_guard.can_activate(&ctx_without_admin_header()).await,
        "guard should reject a request without x-admin-token"
    );
}
