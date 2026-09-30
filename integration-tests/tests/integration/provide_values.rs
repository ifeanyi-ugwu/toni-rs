//! `provide!(key => source)` and the value API it expands to.
//!
//! Every key is a type. A declaration without a key binds under the type it builds and is read
//! back by `#[inject]`; one under a marker is read back by `#[inject(K)]` and the marker's
//! lookups, and one under a marker holding a trait object hands out `Arc<dyn Trait>`. The role
//! cases gate a route: a plain guard value becomes a guard because its marker holds
//! `dyn Guard<HttpContext>`, a type's own declaration keeps its roles under a marker, and
//! contributing to the collection `dyn Guard<HttpContext>` makes it a global HTTP guard. A guard
//! value under a marker holding a data type registers no role.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serial_test::serial;
use ulo::di::{Declaration, DeclaresProvider, Execution, Key, Provide, token_of};
use ulo::enhancer::{Guard, Interceptor, InterceptorNext};
use ulo::http::{Body, HttpContext, HttpHandlerResult};
use ulo::{
    UloFactory, async_trait, controller, get, injectable, key, module, on_module_init, provide,
    routes,
};

use crate::common::TestServer;

const SEVEN: u32 = 7;

async fn make_ten() -> u32 {
    10
}

#[injectable]
pub struct Prefix {
    #[default("app".to_string())]
    pub value: String,
}

#[injectable]
pub struct Logger {
    #[default("logger".to_string())]
    pub name: String,
}

#[derive(Clone)]
pub struct Banner(String);

/// A plain type written as its own key.
#[derive(Clone, Key)]
pub struct Motd(String);

pub trait Greeter: Send + Sync {
    fn greet(&self) -> String;
}

#[injectable]
pub struct EnglishGreeter {
    #[default("hello".to_string())]
    word: String,
}

impl Greeter for EnglishGreeter {
    fn greet(&self) -> String {
        self.word.clone()
    }
}

pub struct QuietGreeter(&'static str);

impl Greeter for QuietGreeter {
    fn greet(&self) -> String {
        self.0.to_string()
    }
}

pub trait Plugin: Send + Sync {
    fn name(&self) -> String;
}

pub struct A {}

impl Plugin for A {
    fn name(&self) -> String {
        "a".to_string()
    }
}

pub struct B(String);

impl Plugin for B {
    fn name(&self) -> String {
        format!("b:{}", self.0)
    }
}

#[injectable]
pub struct C {
    #[default("c".to_string())]
    label: String,
}

impl Plugin for C {
    fn name(&self) -> String {
        self.label.clone()
    }
}

key!(pub Port: u16);
key!(pub PortAlias: u16);
key!(pub Seven: u32);
key!(pub Ten: u32);
key!(pub BannerText: String);
key!(pub Replica: Logger);
key!(pub ReplicaAgain: Logger);
key!(pub Loud: dyn Greeter);
key!(pub Quiet: dyn Greeter);
key!(pub Legacy: dyn Plugin);

/// Holds the collections, and second readers of `Loud` and `Quiet`.
#[injectable]
pub struct PluginRegistry {
    #[inject]
    plugins: Vec<Arc<dyn Plugin>>,
    #[inject]
    bounded: Vec<Arc<dyn Plugin + Send + Sync>>,
    #[inject(Legacy)]
    legacy: Vec<Arc<dyn Plugin>>,
    #[inject(Quiet)]
    quiet: Arc<dyn Greeter>,
    #[inject(Loud)]
    loud: Arc<dyn Greeter>,
}

#[controller("/values")]
pub struct Values {
    #[inject]
    keyless: Banner,
    #[inject]
    motd: Motd,
    #[inject]
    greeter: Arc<dyn Greeter>,
    #[inject]
    bounded_greeter: Arc<dyn Greeter + Send + Sync>,
    #[inject(Port)]
    port: u16,
    #[inject(Replica)]
    replica: Logger,
    #[inject(Loud)]
    loud: Arc<dyn Greeter>,
    #[inject(Quiet)]
    quiet: Arc<dyn Greeter>,
    #[inject]
    registry: PluginRegistry,
}

#[routes]
impl Values {
    #[get("/")]
    fn all(&self) -> Body {
        let names = |ps: Vec<String>| ps.join(",");
        Body::text(format!(
            "{} {} {} {} [{}] [{}] {} {} {} {} [{}] {} {}",
            self.keyless.0,
            self.motd.0,
            self.greeter.greet(),
            self.bounded_greeter.greet(),
            names(self.registry.plugins.iter().map(|p| p.name()).collect()),
            names(self.registry.bounded.iter().map(|p| p.name()).collect()),
            self.port,
            self.replica.name,
            self.loud.greet(),
            self.quiet.greet(),
            names(self.registry.legacy.iter().map(|p| p.name()).collect()),
            Arc::ptr_eq(&self.quiet, &self.registry.quiet),
            Arc::ptr_eq(&self.loud, &self.registry.loud),
        ))
    }
}

#[module(
    controllers: [Values],
    providers: [
        Prefix,
        PluginRegistry,
        provide!(Port => 3000u16),
        provide!(PortAlias => alias Port),
        provide!(Seven => value SEVEN),
        provide!(Ten => factory make_ten),
        provide!(BannerText => async |prefix: Prefix| format!("{}-banner", prefix.value)),
        provide!(async |prefix: Prefix| Banner(format!("{}-keyless", prefix.value))),
        provide!(Motd => Motd("welcome".to_string())),
        provide!(Replica => Logger),
        Logger::provide().under_key::<ReplicaAgain>(),
        provide!(Loud => EnglishGreeter),
        provide!(Quiet => QuietGreeter("psst")),
        provide!(dyn Greeter => EnglishGreeter),
        provide!(dyn Greeter + Send + Sync => EnglishGreeter),
        provide!(into dyn Plugin => A {}),
        provide!(into dyn Plugin => async |prefix: Prefix| B(prefix.value)),
        provide!(into dyn Plugin => C),
        provide!(into Legacy => A {}),
        provide!(into dyn Plugin + Send + Sync => A {}),
    ],
)]
impl ValuesModule {}

#[serial]
#[tokio::test]
async fn each_declaration_is_read_by_the_key_it_was_written_with() {
    let server = TestServer::start(ValuesModule).await;
    let body = server
        .client()
        .get(server.url("/values"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        body,
        "app-keyless welcome hello hello [a,b:app,c] [a] 3000 logger hello psst [a] true true"
    );
}

#[serial]
#[tokio::test]
async fn a_declaration_under_a_marker_is_read_by_the_marker() {
    let app = UloFactory::create(ValuesModule).await.unwrap();

    assert_eq!(*app.get_key::<Port>().await.unwrap(), 3000);
    assert_eq!(*app.get_key::<PortAlias>().await.unwrap(), 3000);
    assert_eq!(*app.get_key::<Seven>().await.unwrap(), 7);
    assert_eq!(*app.get_key::<Ten>().await.unwrap(), 10);
    assert_eq!(*app.get_key::<BannerText>().await.unwrap(), "app-banner");
    assert_eq!(app.get_key::<Motd>().await.unwrap().0, "welcome");
    assert_eq!(app.get_key::<Replica>().await.unwrap().name, "logger");
    assert_eq!(app.get_key::<ReplicaAgain>().await.unwrap().name, "logger");
}

#[serial]
#[tokio::test]
async fn a_marker_is_read_through_a_module() {
    let app = UloFactory::create(ValuesModule).await.unwrap();
    let module = app.get_module::<ValuesModule>().await.unwrap();

    assert_eq!(
        *app.get_from_key::<Port>(module.current_module())
            .await
            .unwrap(),
        3000
    );
    assert_eq!(*module.get_key::<Seven>().execute().await.unwrap(), 7);
    assert_eq!(
        *module
            .resolve_key::<Ten>(&Execution::standalone())
            .execute()
            .await
            .unwrap(),
        10
    );
}

/// A guard that is a plain value: no `#[injectable]`, no dependencies.
#[derive(Clone)]
pub struct HeaderGuard(&'static str);

#[async_trait]
impl Guard<HttpContext> for HeaderGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key(self.0)
    }
}

static BUILT_PER_CALL: AtomicUsize = AtomicUsize::new(0);

/// A guard counting its constructions, to show a per-execution factory builds one per request.
struct CountedGuard;

impl CountedGuard {
    fn new() -> Self {
        BUILT_PER_CALL.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

#[async_trait]
impl Guard<HttpContext> for CountedGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        true
    }
}

static ADMIN_INTERCEPTS: AtomicUsize = AtomicUsize::new(0);

/// A guard and an interceptor with a declaration of its own, whose roles `#[injectable]` detects.
/// Only the guard role is one its marker's `Value` names.
#[injectable]
pub struct AdminGuard {
    #[default("x-admin")]
    header: &'static str,
}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key(self.header)
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for AdminGuard {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        ADMIN_INTERCEPTS.fetch_add(1, Ordering::SeqCst);
        next.run(ctx).await
    }
}

key!(pub Auth: dyn Guard<HttpContext>);
key!(pub PerCall: dyn Guard<HttpContext>);
key!(pub Admin: dyn Guard<HttpContext>);
key!(pub AdminAgain: AdminGuard);

#[controller("/roles")]
pub struct Roles;

#[routes]
impl Roles {
    #[get("/open")]
    fn open(&self) -> Body {
        Body::text("open".to_string())
    }

    #[get("/guarded")]
    #[use_guards(Auth)]
    fn guarded(&self) -> Body {
        Body::text("guarded".to_string())
    }

    #[get("/per-call")]
    #[use_guards(PerCall)]
    fn per_call(&self) -> Body {
        Body::text("per-call".to_string())
    }

    #[get("/admin-again")]
    #[use_guards(AdminAgain)]
    fn admin_again(&self) -> Body {
        Body::text("admin".to_string())
    }

    #[get("/admin")]
    #[use_guards(Admin)]
    #[use_interceptors(Admin)]
    fn admin(&self) -> Body {
        Body::text("admin".to_string())
    }
}

#[module(
    controllers: [Roles],
    providers: [
        provide!(Auth => HeaderGuard("x-auth")),
        provide!(into dyn Guard<HttpContext> => HeaderGuard("x-global")),
        provide!(PerCall => async || CountedGuard::new()).per_execution(),
        provide!(Admin => AdminGuard),
        provide!(AdminAgain => AdminGuard),
    ],
)]
impl RolesModule {}

async fn status(server: &TestServer, path: &str, headers: &[&str]) -> u16 {
    let mut request = server.client().get(server.url(path));
    for header in headers {
        request = request.header(*header, "1");
    }
    request.send().await.unwrap().status().as_u16()
}

#[serial]
#[tokio::test]
async fn a_value_under_a_role_slot_is_that_role() {
    let server = TestServer::start(RolesModule).await;

    assert_eq!(
        status(&server, "/roles/open", &[]).await,
        403,
        "the global guard refuses"
    );
    assert_eq!(status(&server, "/roles/open", &["x-global"]).await, 200);
    assert_eq!(
        status(&server, "/roles/guarded", &["x-global"]).await,
        403,
        "the route guard refuses"
    );
    assert_eq!(
        status(&server, "/roles/guarded", &["x-global", "x-auth"]).await,
        200
    );
}

#[serial]
#[tokio::test]
async fn a_type_under_a_role_slot_keeps_its_own_role() {
    let server = TestServer::start(RolesModule).await;
    let before = ADMIN_INTERCEPTS.load(Ordering::SeqCst);

    assert_eq!(status(&server, "/roles/admin", &["x-global"]).await, 403);
    assert_eq!(
        status(&server, "/roles/admin", &["x-global", "x-admin"]).await,
        200
    );
    assert_eq!(
        ADMIN_INTERCEPTS.load(Ordering::SeqCst) - before,
        1,
        "the interceptor role, which the marker does not name, comes with the type"
    );
}

#[serial]
#[tokio::test]
async fn a_type_under_a_marker_holding_it_keeps_its_roles() {
    let server = TestServer::start(RolesModule).await;

    assert_eq!(
        status(&server, "/roles/admin-again", &["x-global"]).await,
        403
    );
    assert_eq!(
        status(&server, "/roles/admin-again", &["x-global", "x-admin"]).await,
        200
    );
}

#[serial]
#[tokio::test]
async fn a_factory_declared_per_execution_builds_its_guard_per_request() {
    let server = TestServer::start(RolesModule).await;
    let before = BUILT_PER_CALL.load(Ordering::SeqCst);

    for _ in 0..2 {
        assert_eq!(status(&server, "/roles/per-call", &["x-global"]).await, 200);
    }

    assert_eq!(BUILT_PER_CALL.load(Ordering::SeqCst) - before, 2);
}

key!(pub Untyped: HeaderGuard);

#[controller("/untyped")]
pub struct UntypedRoute;

#[routes]
impl UntypedRoute {
    #[get("/")]
    #[use_guards(Untyped)]
    fn guarded(&self) -> Body {
        Body::text("guarded".to_string())
    }
}

#[module(
    controllers: [UntypedRoute],
    providers: [provide!(Untyped => HeaderGuard("x-auth"))],
)]
impl UntypedModule {}

#[serial]
#[tokio::test]
async fn a_guard_under_a_marker_holding_a_data_type_registers_no_role() {
    let Err(error) = UloFactory::create(UntypedModule).await else {
        panic!("a guard under a marker holding `HeaderGuard` must not register as a guard");
    };
    let message = error.to_string();
    assert!(
        message.contains(&format!(
            "HTTP Guard '{}' not found in registry",
            token_of::<Untyped>()
        )),
        "{message}"
    );
}

#[derive(Clone)]
struct RpcGuard;

#[async_trait]
impl Guard<ulo::rpc::RpcContext> for RpcGuard {
    async fn can_activate(&self, _ctx: &ulo::rpc::RpcContext) -> bool {
        true
    }
}

#[module(providers: [provide!(into dyn Guard<ulo::rpc::RpcContext> => value RpcGuard)])]
impl UnreadGlobalModule {}

#[serial]
#[tokio::test]
async fn a_role_collection_no_global_set_reads_is_refused() {
    let Err(error) = UloFactory::create(UnreadGlobalModule).await else {
        panic!("a global RPC guard contributed to a collection must be refused, not dropped");
    };
    let message = error.to_string();
    assert!(message.contains("use_global_rpc_guards"), "{message}");
}

#[test]
fn a_declaration_carries_the_key_it_was_written_with() {
    use ulo::spi::ProviderFactory;

    assert_eq!(Provide::value(1u16).token(), token_of::<u16>());
    assert_eq!(Provide::value_key::<Port>(1).token(), token_of::<Port>());
    assert_eq!(provide!(Port => 1u16).token(), token_of::<Port>());
    assert_eq!(
        provide!(Quiet => QuietGreeter("")).token(),
        token_of::<Quiet>()
    );
    assert_eq!(
        Provide::alias_key::<PortAlias, Port>().dependency_tokens(),
        [token_of::<Port>()]
    );
    assert_eq!(
        Provide::factory(async |_: Prefix| String::new()).dependency_tokens(),
        [token_of::<Prefix>()]
    );
    assert_eq!(
        provide!(into dyn Plugin => A {}).multi_base_token(),
        Some(token_of::<dyn Plugin>())
    );
    assert_eq!(
        provide!(into Legacy => A {}).multi_base_token(),
        Some(token_of::<Legacy>())
    );
}

static REQUEST_IDS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct RequestId(usize);

fn next_id() -> RequestId {
    RequestId(REQUEST_IDS.fetch_add(1, Ordering::SeqCst))
}

key!(pub Primary: RequestId);
key!(pub Secondary: RequestId);
key!(pub Fresh: RequestId);

/// A second consumer in the same execution, built apart from the controller, so the two only agree
/// if the execution holds one instance.
#[injectable(scope = "execution")]
pub struct Holder {
    #[inject]
    id: RequestId,
}

#[controller("/per-execution", scope = "execution")]
pub struct PerExecution {
    #[inject]
    direct: RequestId,
    #[inject]
    holder: Holder,
}

#[routes]
impl PerExecution {
    #[get("/")]
    fn ids(&self) -> Body {
        Body::text(format!("{} {}", self.direct.0, self.holder.id.0))
    }
}

#[module(
    controllers: [PerExecution],
    providers: [
        Holder,
        provide!(async || next_id()).per_execution(),
        Provide::factory(async || next_id()).under_key::<Primary>().per_execution(),
        Provide::factory(async || next_id()).under_key::<Secondary>().per_execution(),
        Provide::factory(async || next_id()).under_key::<Fresh>().transient(),
    ],
)]
impl PerExecutionModule {}

async fn request_ids(server: &TestServer) -> (String, String) {
    let body = server
        .client()
        .get(server.url("/per-execution"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (first, second) = body.split_once(' ').unwrap();
    (first.to_string(), second.to_string())
}

#[serial]
#[tokio::test]
async fn a_factory_declared_per_execution_is_shared_within_one_request() {
    let server = TestServer::start(PerExecutionModule).await;

    let (a1, b1) = request_ids(&server).await;
    let (a2, b2) = request_ids(&server).await;
    assert_eq!(a1, b1, "one request shares one instance");
    assert_eq!(a2, b2, "one request shares one instance");
    assert_ne!(a1, a2, "each request builds its own");
}

#[serial]
#[tokio::test]
async fn two_markers_over_one_type_share_no_instance() {
    let app = UloFactory::create(PerExecutionModule).await.unwrap();
    let execution = Execution::standalone();

    let primary = app.resolve_key::<Primary>(&execution).await.unwrap().0;
    let again = app.resolve_key::<Primary>(&execution).await.unwrap().0;
    let secondary = app.resolve_key::<Secondary>(&execution).await.unwrap().0;
    assert_eq!(primary, again, "one execution shares one instance");
    assert_ne!(
        primary, secondary,
        "two declarations of one type share none"
    );
}

#[serial]
#[tokio::test]
async fn a_transient_factory_builds_at_every_resolution() {
    let app = UloFactory::create(PerExecutionModule).await.unwrap();
    let execution = Execution::standalone();

    let first = app.resolve_key::<Fresh>(&execution).await.unwrap().0;
    let second = app.resolve_key::<Fresh>(&execution).await.unwrap().0;
    assert_ne!(first, second);
}

static COUNTED_INITS: AtomicUsize = AtomicUsize::new(0);
static PLUGIN_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
pub struct Counted {
    #[default(0)]
    _marker: u8,
}

impl Counted {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        COUNTED_INITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
pub struct HookedPlugin {
    #[default(0)]
    _marker: u8,
}

impl HookedPlugin {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        PLUGIN_INITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Plugin for HookedPlugin {
    fn name(&self) -> String {
        "hooked".to_string()
    }
}

key!(pub CountedAlias: Counted);

#[module(
    providers: [
        Counted,
        provide!(CountedAlias => alias Counted),
        provide!(into dyn Plugin => HookedPlugin),
    ],
)]
impl HooksModule {}

static REBOUND_INITS: AtomicUsize = AtomicUsize::new(0);

pub trait Tally: Send + Sync {}

#[injectable]
pub struct Rebound {
    #[default(0)]
    _marker: u8,
}

impl Rebound {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        REBOUND_INITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Tally for Rebound {}

key!(pub ReboundAgain: Rebound);

#[module(providers: [provide!(ReboundAgain => Rebound)])]
impl RekeyedHooksModule {}

#[module(providers: [provide!(dyn Tally => Rebound)])]
impl RecastHooksModule {}

#[serial]
#[tokio::test]
async fn an_alias_runs_no_second_set_of_hooks() {
    let before = COUNTED_INITS.load(Ordering::SeqCst);
    let _app = UloFactory::create(HooksModule).await.unwrap();
    assert_eq!(COUNTED_INITS.load(Ordering::SeqCst) - before, 1);
}

#[serial]
#[tokio::test]
async fn a_type_rebound_under_another_key_runs_its_hooks() {
    let before = REBOUND_INITS.load(Ordering::SeqCst);
    let _app = UloFactory::create(RekeyedHooksModule).await.unwrap();
    assert_eq!(
        REBOUND_INITS.load(Ordering::SeqCst) - before,
        1,
        "under a marker"
    );

    let before = REBOUND_INITS.load(Ordering::SeqCst);
    let _app = UloFactory::create(RecastHooksModule).await.unwrap();
    assert_eq!(
        REBOUND_INITS.load(Ordering::SeqCst) - before,
        1,
        "under a trait object"
    );
}

#[serial]
#[tokio::test]
async fn a_contribution_built_from_a_declaration_runs_its_hooks() {
    let before = PLUGIN_INITS.load(Ordering::SeqCst);
    let _app = UloFactory::create(HooksModule).await.unwrap();
    assert_eq!(PLUGIN_INITS.load(Ordering::SeqCst) - before, 1);
}
