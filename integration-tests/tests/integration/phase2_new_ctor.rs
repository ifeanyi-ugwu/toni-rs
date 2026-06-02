//! `#[new]` constructor injection on `#[derive(Injectable)]`: a dependency can be a constructor
//! parameter without being a stored field, and the constructor can derive non-injected state.
//!
//! This is the one case field injection can't express — `Server` injects `ConfigService`, keeps
//! only a derived `port`, and never stores the config.

use std::sync::Arc;

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::{
    Body as ToniBody, Injectable, controller, get, module, new, toni_factory::ToniFactory,
    use_guards,
};

use crate::common::TestServer;
use serial_test::serial;

#[derive(Clone, Injectable)]
pub struct ConfigService {
    #[default(8080)]
    port: u16,
}

impl ConfigService {
    pub fn port(&self) -> u16 {
        self.port
    }
}

// `#[new]`: ConfigService is injected and used, but NOT a field. `port` is derived from it. The
// struct has no DI field at all — impossible to express with field injection.
#[derive(Clone, Injectable)]
pub struct Server {
    port: u16,
}

impl Server {
    #[new]
    fn new(config: ConfigService) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

// A guard built via #[new] with a non-stored dep — proves ctor-built providers still get their
// roles auto-detected (the bridge feeds the same factory the probes run in).
#[derive(Clone, Injectable)]
pub struct PortGuard {
    threshold: u16,
}

impl PortGuard {
    #[new]
    fn new(config: ConfigService) -> Self {
        Self {
            threshold: config.port(),
        }
    }
}

#[async_trait]
impl Guard<HttpContext> for PortGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        // admit only when the caller echoes the configured port in a header
        ctx.request()
            .headers
            .get("x-port")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u16>().ok())
            == Some(self.threshold)
    }
}

// Request-scoped #[new]: built fresh per request, ConfigService injected, not stored.
#[derive(Clone, Injectable)]
#[provider(scope = "request")]
pub struct ReqServer {
    port: u16,
}

impl ReqServer {
    #[new]
    fn new(config: ConfigService) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

// Transient-scoped #[new]: built per resolution.
#[derive(Clone, Injectable)]
#[provider(scope = "transient")]
pub struct TransientServer {
    port: u16,
}

impl TransientServer {
    #[new]
    fn new(config: ConfigService) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

#[derive(Clone)]
pub struct ApiController;

#[controller("/srv")]
impl ApiController {
    pub fn new() -> Self {
        Self
    }

    #[get("/guarded")]
    #[use_guards(PortGuard)]
    fn guarded(&self) -> ToniBody {
        ToniBody::text("ok".to_string())
    }
}

// Injects the request-scoped #[new] provider as a field (the request-scoped DI path) and echoes its
// derived port — proving the constructor ran per request, not that a defaulted field was used.
#[controller("/req", pub struct ReqController {
    #[inject]
    server: ReqServer,
})]
impl ReqController {
    #[get("/port")]
    fn port(&self) -> ToniBody {
        ToniBody::text(self.server.port().to_string())
    }
}

#[module(
    controllers: [ApiController, ReqController],
    providers: [ConfigService, Server, PortGuard, ReqServer, TransientServer],
)]
struct NewCtorModule {}

#[tokio_localset_test::localset_test]
async fn new_ctor_injects_without_storing() {
    let app = ToniFactory::create_application_context(NewCtorModule::module_definition()).await;

    // Server was built via Self::new(config) — config injected, only port kept.
    let server: Server = app.get::<Server>().await.expect("Server resolves via #[new]");
    assert_eq!(server.port(), 8080);
}

#[serial]
#[tokio_localset_test::localset_test]
async fn new_ctor_built_guard_still_auto_detects_role() {
    let server = TestServer::start(NewCtorModule::module_definition()).await;

    let resp = server
        .client()
        .get(server.url("/srv/guarded"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "guard blocks without matching x-port");

    let resp = server
        .client()
        .get(server.url("/srv/guarded"))
        .header("X-Port", "8080")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "guard admits when x-port matches configured port");
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio_localset_test::localset_test]
async fn new_ctor_transient_scope_resolves() {
    let app = ToniFactory::create_application_context(NewCtorModule::module_definition()).await;
    // Transient is resolvable through the application context; the constructor must have run.
    let t: TransientServer = app
        .get::<TransientServer>()
        .await
        .expect("TransientServer resolves via #[new]");
    assert_eq!(t.port(), 8080);
}

#[serial]
#[tokio_localset_test::localset_test]
async fn new_ctor_request_scope_resolves_per_request() {
    let server = TestServer::start(NewCtorModule::module_definition()).await;
    let resp = server
        .client()
        .get(server.url("/req/port"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "8080",
        "request-scoped #[new] must run the constructor (injected port), not default the field"
    );
}

// Keep Arc import meaningful even if unused in asserts.
#[allow(dead_code)]
fn _arc_marker(_: Arc<()>) {}
