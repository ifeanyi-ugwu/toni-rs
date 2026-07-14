//! End-to-end proof that a guard needs no marker: `#[injectable]` + `impl Guard<HttpContext>`,
//! applied with `#[use_guards(AdminGuard)]`, blocks/admits real HTTP requests.
//!
//! The provider factory auto-detects the `Guard<HttpContext>` impl (via `toni::__detect`) and
//! registers the role; `#[use_guards]` and the role registry resolve it unchanged. Contrast
//! `enhancers_di.rs`, where the equivalent guard still carries `#[injectable(struct …)]` +
//! `#[guard(http)]`.

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::{
    controller, get, injectable, module, routes, use_guards, Body as ToniBody, RequestPart,
};

use crate::common::TestServer;
use serial_test::serial;

#[injectable]
pub struct AuthService {
    #[default(true)]
    require_token: bool,
}

impl AuthService {
    fn is_admin(&self, req: &RequestPart) -> bool {
        !self.require_token || req.headers.contains_key("x-admin-token")
    }
}

// No `#[guard(http)]`. The `impl Guard<HttpContext>` below is the only declaration.
#[injectable]
pub struct AdminGuard {
    #[inject]
    auth: AuthService,
}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        self.auth.is_admin(ctx.request())
    }
}

// A request-scoped guard, also marker-free: `#[injectable(scope = "request")]` sets the
// scope; the `impl Guard<HttpContext>` is detected per request through the dyn-factory path. This
// exercises the type-level probe (registration decision) + value probe (per-request coercion).
#[injectable(scope = "request")]
pub struct RequestScopedGuard {
    #[default(false)]
    _per_request: bool,
}

#[async_trait]
impl Guard<HttpContext> for RequestScopedGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-allow")
    }
}

#[controller("/api")]
pub struct ApiController;

#[routes]
impl ApiController {
    #[get("/admin")]
    #[use_guards(AdminGuard)]
    fn admin(&self) -> ToniBody {
        ToniBody::text("admin ok".to_string())
    }

    #[get("/scoped")]
    #[use_guards(RequestScopedGuard)]
    fn scoped(&self) -> ToniBody {
        ToniBody::text("scoped ok".to_string())
    }
}

#[module(
    controllers: [ApiController],
    providers: [AuthService, AdminGuard, RequestScopedGuard],
)]
struct MarkerFreeModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn marker_free_guard_blocks_and_admits_over_http() {
    let server = TestServer::start(MarkerFreeModule).await;

    // No token: the marker-free guard rejects.
    let resp = server
        .client()
        .get(server.url("/api/admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "guard must block without x-admin-token");

    // With token: the guard admits and the handler runs.
    let resp = server
        .client()
        .get(server.url("/api/admin"))
        .header("X-Admin-Token", "valid")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "guard must admit with x-admin-token");
    assert_eq!(resp.text().await.unwrap(), "admin ok");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn marker_free_request_scoped_guard_blocks_and_admits() {
    let server = TestServer::start(MarkerFreeModule).await;

    let resp = server
        .client()
        .get(server.url("/api/scoped"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        403,
        "request-scoped guard must block without x-allow"
    );

    let resp = server
        .client()
        .get(server.url("/api/scoped"))
        .header("X-Allow", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "request-scoped guard must admit with x-allow"
    );
    assert_eq!(resp.text().await.unwrap(), "scoped ok");
}
