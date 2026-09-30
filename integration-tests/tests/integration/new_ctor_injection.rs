//! `#[new]` constructor injection on `#[injectable]`: a dependency can be a constructor
//! parameter without being a stored field, and the constructor can derive non-injected state.
//!
//! This is the one case field injection can't express — `Server` injects `ConfigService`, keeps
//! only a derived `port`, and never stores the config.

use std::sync::Arc;

use crate::common::TestServer;
use serial_test::serial;
use ulo::async_trait;
use ulo::enhancer::Guard;
use ulo::http::Body;
use ulo::http::HttpContext;
use ulo::{UloFactory, controller, get, injectable, module, new, routes, use_guards};
#[injectable]
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
#[injectable]
pub struct Server {
    port: u16,
}

impl Server {
    #[new]
    fn new(config: Arc<ConfigService>) -> Self {
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
#[injectable]
pub struct PortGuard {
    threshold: u16,
}

impl PortGuard {
    #[new]
    fn new(config: Arc<ConfigService>) -> Self {
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

// Execution-scoped #[new]: built fresh per request, ConfigService injected, not stored.
#[injectable(scope = "execution")]
pub struct ReqServer {
    port: u16,
}

impl ReqServer {
    #[new]
    fn new(config: Arc<ConfigService>) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

// The case the request-context fix unlocks: an execution-scoped #[new] provider whose constructor
// injects ANOTHER execution-scoped provider (ReqServer). Before the fix this panicked, because the
// constructor resolved its params with no request context.
#[injectable(scope = "execution")]
pub struct ReqFacade {
    port: u16,
}

impl ReqFacade {
    #[new]
    fn new(inner: Arc<ReqServer>) -> Self {
        Self { port: inner.port() }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

// Transient-scoped #[new]: built per resolution.
#[injectable(scope = "transient")]
pub struct TransientServer {
    port: u16,
}

impl TransientServer {
    #[new]
    fn new(config: Arc<ConfigService>) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

// A stored field whose type has NO `Default` impl, built entirely by `#[new]`. The factory always
// emits a field-injection construction path even when a constructor exists (the macros can't see
// each other), so this field would force `Default` if that dead path defaulted it directly. The
// constructor builds it here; the field stays plain.
#[derive(Clone)]
pub struct Handle(String);

#[injectable]
pub struct ConnHolder {
    handle: Handle,
}

impl ConnHolder {
    #[new]
    fn new(config: Arc<ConfigService>) -> Self {
        Self {
            handle: Handle(format!("conn:{}", config.port())),
        }
    }

    pub fn handle(&self) -> &str {
        &self.handle.0
    }
}

// `#[inject]` on a `#[new]` parameter: read for the lookup token, then stripped from the re-emitted
// constructor. Left in place, rustc rejects `#[inject]` as an unknown attribute on a fn parameter.
#[injectable]
pub struct ExplicitInjectServer {
    port: u16,
}

impl ExplicitInjectServer {
    #[new]
    fn new(#[inject] config: Arc<ConfigService>) -> Self {
        Self {
            port: config.port(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

#[controller("/srv")]
pub struct ApiController;

#[routes]
impl ApiController {
    #[get("/guarded")]
    #[use_guards(PortGuard)]
    fn guarded(&self) -> Body {
        Body::text("ok".to_string())
    }
}

// Injects the execution-scoped #[new] provider as a field (the execution-scoped DI path) and echoes its
// derived port — proving the constructor ran per request, not that a defaulted field was used.
#[controller("/req")]
pub struct ReqController {
    #[inject]
    server: Arc<ReqServer>,
    #[inject]
    facade: Arc<ReqFacade>,
}

#[routes]
impl ReqController {
    #[get("/port")]
    fn port(&self) -> Body {
        Body::text(self.server.port().to_string())
    }

    // ReqFacade was built via #[new] injecting the execution-scoped ReqServer — exercises the
    // request-context threading through the constructor bridge.
    #[get("/facade-port")]
    fn facade_port(&self) -> Body {
        Body::text(self.facade.port().to_string())
    }
}

#[module(
    controllers: [ApiController, ReqController],
    providers: [
        ConfigService,
        Server,
        PortGuard,
        ReqServer,
        TransientServer,
        ReqFacade,
        ConnHolder,
        ExplicitInjectServer,
    ],
)]
struct NewCtorModule {}

#[tokio::test]
async fn new_ctor_injects_without_storing() {
    let app = UloFactory::create_application_context(NewCtorModule)
        .await
        .unwrap();

    // Server was built via Self::new(config) — config injected, only port kept.
    let server = app
        .get::<Server>()
        .await
        .expect("Server resolves via #[new]");
    assert_eq!(server.port(), 8080);
}

#[serial]
#[tokio::test]
async fn new_ctor_built_guard_still_auto_detects_role() {
    let server = TestServer::start(NewCtorModule).await;

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
    assert_eq!(
        resp.status(),
        200,
        "guard admits when x-port matches configured port"
    );
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn new_ctor_transient_scope_resolves() {
    let app = UloFactory::create_application_context(NewCtorModule)
        .await
        .unwrap();
    // Transient is resolvable through the application context; the constructor must have run.
    let t = app
        .get::<TransientServer>()
        .await
        .expect("TransientServer resolves via #[new]");
    assert_eq!(t.port(), 8080);
}

#[serial]
#[tokio::test]
async fn new_ctor_request_scope_resolves_per_request() {
    let server = TestServer::start(NewCtorModule).await;
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
        "execution-scoped #[new] must run the constructor (injected port), not default the field"
    );
}

#[serial]
#[tokio::test]
async fn new_ctor_can_inject_request_scoped_dependency() {
    let server = TestServer::start(NewCtorModule).await;
    // ReqFacade's #[new] injects the execution-scoped ReqServer — resolves only because the
    // constructor bridge threads the request context.
    let resp = server
        .client()
        .get(server.url("/req/facade-port"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "8080");
}

#[tokio::test]
async fn new_ctor_builds_non_default_field() {
    let app = UloFactory::create_application_context(NewCtorModule)
        .await
        .unwrap();
    // `Handle` has no `Default`; the field is built solely by the constructor. Resolving proves the
    // dead field-injection path compiles without a `Default` bound and the constructor runs.
    let holder = app
        .get::<ConnHolder>()
        .await
        .expect("ConnHolder resolves via #[new] despite a non-Default field");
    assert_eq!(holder.handle(), "conn:8080");
}

#[tokio::test]
async fn new_ctor_strips_inject_attr_from_params() {
    let app = UloFactory::create_application_context(NewCtorModule)
        .await
        .unwrap();
    let server = app
        .get::<ExplicitInjectServer>()
        .await
        .expect("ExplicitInjectServer resolves via #[new] with an #[inject] parameter");
    assert_eq!(server.port(), 8080);
}

// Keep Arc import meaningful even if unused in asserts.
#[allow(dead_code)]
fn _arc_marker(_: Arc<()>) {}

// The path-qualified spelling of `#[inject]` on a `#[new]` parameter must be read and
// stripped like the bare one — unmatched it is neither token-routed nor removed.
#[tokio::test]
async fn new_ctor_path_qualified_inject_token() {
    ulo::key!(Greeting: String);

    #[injectable]
    pub struct Greeter {
        greeting: Arc<String>,
    }

    impl Greeter {
        #[new]
        fn new(#[ulo::inject(Greeting)] greeting: Arc<String>) -> Self {
            Self { greeting }
        }

        pub fn greeting(&self) -> &str {
            &self.greeting
        }
    }

    #[module(providers: [
        ulo::provide!(Greeting => "hello".to_string()),
        Greeter,
    ])]
    struct GreetModule {}

    let app = UloFactory::create_application_context(GreetModule)
        .await
        .unwrap();
    let greeter = app.get::<Greeter>().await.expect("Greeter resolves");
    assert_eq!(greeter.greeting(), "hello");
}
