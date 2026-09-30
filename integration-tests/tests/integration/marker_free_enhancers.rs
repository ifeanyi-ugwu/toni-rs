//! A guard is declared by implementing `Guard<HttpContext>` and by nothing
//! else: `#[injectable]` on the struct, `#[use_guards(AdminGuard)]` on the
//! handler impl, and it blocks or admits real HTTP requests.
//!
//! The provider factory reads the trait impl off the concrete type where
//! `build()` knows it (`ulo::__detect`) and registers the role from that, so
//! the role registry resolves a guard its author never labelled. Both scopes
//! are covered, because they detect at different points: a singleton at
//! construction, an execution-scoped guard on each request.

use crate::common::TestServer;
use serial_test::serial;
use std::sync::Arc;
use ulo::async_trait;
use ulo::enhancer::Guard;
use ulo::http::HttpContext;
use ulo::http::{Body, RequestPart};
use ulo::{controller, get, injectable, module, routes, use_guards};
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

// The `impl Guard<HttpContext>` below is the only thing that makes this a guard.
#[injectable]
pub struct AdminGuard {
    #[inject]
    auth: Arc<AuthService>,
}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        self.auth.is_admin(ctx.request())
    }
}

// An execution-scoped guard, also marker-free: `#[injectable(scope = "execution")]` sets the
// scope; the `impl Guard<HttpContext>` is detected per request through the dyn-factory path. This
// exercises the type-level probe (registration decision) + value probe (per-request coercion).
#[injectable(scope = "execution")]
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
    fn admin(&self) -> Body {
        Body::text("admin ok".to_string())
    }

    #[get("/scoped")]
    #[use_guards(RequestScopedGuard)]
    fn scoped(&self) -> Body {
        Body::text("scoped ok".to_string())
    }
}

#[module(
    controllers: [ApiController],
    providers: [AuthService, AdminGuard, RequestScopedGuard],
)]
struct MarkerFreeModule {}

#[serial]
#[tokio::test]
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
#[tokio::test]
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
        "execution-scoped guard must block without x-allow"
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
        "execution-scoped guard must admit with x-allow"
    );
    assert_eq!(resp.text().await.unwrap(), "scoped ok");
}
