//! Which scope may inject which: a singleton cannot depend on an execution-scoped
//! provider, and anything may depend on a transient one.
//!
//! A singleton outlives every request, so holding something execution-scoped
//! means holding one arbitrary request's state forever. That failure is
//! invisible at runtime — the application serves correctly until two requests
//! disagree — so `create` refuses it, naming the provider built at startup and
//! the execution-scoped one it reaches, through however many transients.
use std::sync::Arc;

use ulo::di::{Execution, ModuleMetadata, ResolutionError};
use ulo::enhancer::Guard;
use ulo::http::{Body, HttpContext};
use ulo::{
    StartupError, UloFactory, async_trait, controller, get, injectable, key, module, new, provide,
    routes,
};

use crate::common::TestServer;

/// The provider `create` refused to build and the execution-scoped provider it reached.
async fn refusal(module: impl ModuleMetadata + 'static) -> (String, String) {
    match UloFactory::create(module).await {
        Err(StartupError::BuildFailed { token, source, .. }) => {
            match source.downcast_ref::<ResolutionError>() {
                Some(ResolutionError::ExecutionRequired { token: needed }) => {
                    (token, needed.clone())
                }
                _ => panic!("unexpected build failure: {source}"),
            }
        }
        Err(other) => panic!("unexpected startup error: {other}"),
        Ok(_) => panic!("the module must be refused"),
    }
}
#[tokio::test]
async fn valid_singleton_injects_singleton() {
    #[injectable]
    pub struct ServiceA {}
    impl ServiceA {}

    #[injectable]
    pub struct ServiceB {
        #[inject]
        dep: Arc<ServiceA>,
    }
    impl ServiceB {}

    #[module(providers: [ServiceA, ServiceB])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    app.get::<ServiceB>()
        .await
        .expect("ServiceB with ServiceA dep should resolve");
}

#[tokio::test]
async fn valid_request_injects_singleton() {
    #[injectable]
    pub struct SingletonService {}
    impl SingletonService {}

    #[injectable(scope = "execution")]
    pub struct RequestService {
        #[inject]
        dep: Arc<SingletonService>,
    }
    impl RequestService {}

    #[module(providers: [SingletonService, RequestService])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    let execution = Execution::standalone();
    app.resolve::<RequestService>(&execution)
        .await
        .expect("execution-scoped service with singleton dep should resolve");
}

#[tokio::test]
async fn valid_transient_injects_any_scope() {
    #[injectable]
    pub struct SingletonService {}
    impl SingletonService {}

    #[injectable(scope = "execution")]
    pub struct RequestService {}
    impl RequestService {}

    #[injectable(scope = "transient")]
    pub struct TransientService {
        #[inject]
        singleton: Arc<SingletonService>,
        #[inject]
        request: Arc<RequestService>,
    }
    impl TransientService {}

    #[module(providers: [SingletonService, RequestService, TransientService])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    let execution = Execution::standalone();
    app.resolve::<TransientService>(&execution)
        .await
        .expect("transient with mixed deps should resolve");
}

#[tokio::test]
async fn singleton_cannot_inject_request_scoped() {
    #[injectable(scope = "execution")]
    pub struct RequestService {}
    impl RequestService {}

    #[injectable]
    pub struct SingletonService {
        #[inject]
        request_dep: Arc<RequestService>,
    }
    impl SingletonService {}

    #[module(providers: [RequestService, SingletonService])]
    impl InvalidModule {}

    let err = match UloFactory::create(InvalidModule).await {
        Err(err) => err,
        Ok(_) => panic!("the module must be refused"),
    };
    let StartupError::BuildFailed { module, token, .. } = &err else {
        panic!("unexpected startup error: {err}");
    };
    assert!(module.ends_with("InvalidModule"), "{module}");
    assert!(token.ends_with("::SingletonService"), "{token}");
    let message = err.to_string();
    assert!(
        message.contains("SingletonService") && message.contains("RequestService"),
        "{message}"
    );
}

#[tokio::test]
async fn singleton_can_inject_transient() {
    #[injectable(scope = "transient")]
    pub struct TransientService {}
    impl TransientService {}

    #[injectable]
    pub struct SingletonService {
        #[inject]
        transient_dep: TransientService,
    }
    impl SingletonService {}

    #[module(providers: [TransientService, SingletonService])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    app.get::<SingletonService>()
        .await
        .expect("singleton with transient dep should resolve");
}

#[tokio::test]
async fn request_can_inject_transient() {
    #[injectable(scope = "transient")]
    pub struct TransientService {}
    impl TransientService {}

    #[injectable(scope = "execution")]
    pub struct RequestService {
        #[inject]
        transient_dep: TransientService,
    }
    impl RequestService {}

    #[module(providers: [TransientService, RequestService])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    let execution = Execution::standalone();
    app.resolve::<RequestService>(&execution)
        .await
        .expect("execution-scoped with transient dep should resolve");
}

#[tokio::test]
async fn complex_valid_hierarchy() {
    #[injectable]
    pub struct BaseService {}
    impl BaseService {}

    #[injectable]
    pub struct MiddleService {
        #[inject]
        base: Arc<BaseService>,
    }
    impl MiddleService {}

    #[injectable(scope = "execution")]
    pub struct TopService {
        #[inject]
        middle: Arc<MiddleService>,
        #[inject]
        base: Arc<BaseService>,
    }
    impl TopService {}

    #[module(providers: [BaseService, MiddleService, TopService])]
    impl TestModule {}

    let app = UloFactory::create(TestModule).await.unwrap();
    let execution = Execution::standalone();
    app.resolve::<TopService>(&execution)
        .await
        .expect("three-level hierarchy should resolve");
}

#[tokio::test]
async fn explicit_singleton_with_request_fails() {
    #[injectable(scope = "execution")]
    pub struct RequestService {}
    impl RequestService {}

    #[injectable(scope = "singleton")]
    pub struct ExplicitSingleton {
        #[inject]
        request_dep: Arc<RequestService>,
    }
    impl ExplicitSingleton {}

    #[module(providers: [RequestService, ExplicitSingleton])]
    impl TestModule {}

    let (refused, needed) = refusal(TestModule).await;
    assert!(refused.ends_with("::ExplicitSingleton"), "{refused}");
    assert!(needed.ends_with("::RequestService"), "{needed}");
}

#[injectable(scope = "execution")]
pub struct PerCall {}

#[injectable(scope = "transient")]
pub struct Between {
    #[inject]
    per_call: Arc<PerCall>,
}

#[injectable]
pub struct Reaches {
    #[inject]
    between: Between,
}

#[module(providers: [PerCall, Between, Reaches])]
struct ThroughATransient;

#[tokio::test]
async fn a_singleton_reaching_one_through_a_transient_is_refused() {
    let (refused, needed) = refusal(ThroughATransient).await;
    assert!(refused.ends_with("::Reaches"), "{refused}");
    assert!(needed.ends_with("::PerCall"), "{needed}");
}

#[injectable]
pub struct Constructed {
    per_call: Arc<PerCall>,
}

impl Constructed {
    #[new]
    fn new(per_call: Arc<PerCall>) -> Self {
        Self { per_call }
    }
}

#[module(providers: [PerCall, Constructed])]
struct ThroughAConstructor;

#[tokio::test]
async fn a_constructor_parameter_is_refused_like_a_field() {
    let (refused, needed) = refusal(ThroughAConstructor).await;
    assert!(refused.ends_with("::Constructed"), "{refused}");
    assert!(needed.ends_with("::PerCall"), "{needed}");
}

#[derive(Clone)]
pub struct Report;

#[module(providers: [PerCall, provide!(async |_per_call: Arc<PerCall>| Report)])]
struct ThroughAFactory;

#[derive(Clone)]
pub struct Stamp;

#[derive(Clone)]
pub struct Stamped;

#[module(providers: [
    provide!(async || Stamp).per_execution(),
    provide!(async |_stamp: Arc<Stamp>| Stamped),
])]
struct OverAPerExecutionFactory;

key!(pub PerCallPort: u16);

key!(pub Doubled: u32);

#[injectable]
pub struct Dials {
    #[inject(PerCallPort)]
    port: Arc<u16>,
}

#[module(providers: [provide!(PerCallPort => async || 3000u16).per_execution(), Dials])]
struct OverAKeyedPerExecutionFactory;

#[injectable]
pub struct Halves {
    #[inject(Doubled)]
    doubled: u32,
}

#[module(providers: [
    provide!(async || 21u32).per_execution(),
    provide!(Doubled => async |n: Arc<u32>| *n * 2).transient(),
    Halves,
])]
struct ThroughAKeyedTransient;

#[tokio::test]
async fn a_factory_declaration_is_refused_like_a_provider() {
    let (refused, needed) = refusal(ThroughAFactory).await;
    assert!(refused.ends_with("::Report"), "{refused}");
    assert!(needed.ends_with("::PerCall"), "{needed}");

    // A `.per_execution()` declaration reached as a factory's parameter refuses like an
    // `#[injectable]`.
    let (refused, needed) = refusal(OverAPerExecutionFactory).await;
    assert!(refused.ends_with("::Stamped"), "{refused}");
    assert!(needed.ends_with("::Stamp"), "{needed}");

    // The refusal names the key the declaration is bound under, not the type it builds.
    let (refused, needed) = refusal(OverAKeyedPerExecutionFactory).await;
    assert!(refused.ends_with("::Dials"), "{refused}");
    assert!(needed.ends_with("::PerCallPort"), "{needed}");

    // A transient under a key passes on its dependency's refusal, naming the dependency.
    let (refused, needed) = refusal(ThroughAKeyedTransient).await;
    assert!(refused.ends_with("::Halves"), "{refused}");
    assert_eq!(needed, "u32");
}

pub trait Named: Send + Sync {}

#[injectable(scope = "execution")]
pub struct PerCallNamed {}

impl Named for PerCallNamed {}

#[injectable]
pub struct Greets {
    #[inject]
    named: Arc<dyn Named>,
}

#[module(providers: [
    provide!(dyn Named => async |_per_call: Arc<PerCall>| PerCallNamed {}),
    PerCall,
    Greets,
])]
struct ThroughASlotFactory;

#[module(providers: [provide!(dyn Named => PerCallNamed), Greets])]
struct ThroughASlotType;

#[module(providers: [provide!(into dyn Named => PerCallNamed)])]
struct ThroughAContribution;

#[tokio::test]
async fn a_trait_object_slot_or_collection_is_refused_like_a_provider() {
    let (refused, needed) = refusal(ThroughASlotFactory).await;
    assert!(refused.ends_with("::Named"), "{refused}");
    assert!(needed.ends_with("::PerCall"), "{needed}");

    let (refused, needed) = refusal(ThroughASlotType).await;
    assert!(refused.ends_with("::Greets"), "{refused}");
    assert!(needed.ends_with("::PerCallNamed"), "{needed}");

    let (refused, needed) = refusal(ThroughAContribution).await;
    assert!(refused.contains("Named"), "{refused}");
    assert!(needed.ends_with("::PerCallNamed"), "{needed}");
}

#[controller("/reaches")]
pub struct ReachesController {
    #[inject]
    between: Between,
}

#[routes]
impl ReachesController {
    #[get("/")]
    fn show(&self) -> Body {
        let _ = &self.between;
        Body::text("built")
    }
}

#[module(controllers: [ReachesController], providers: [PerCall, Between])]
struct ControllerThroughATransient;

// A controller injecting an execution-scoped provider directly is built per call instead; one
// reaching it through a transient is built once, and refused.
#[tokio::test]
async fn a_controller_built_once_is_refused_like_a_provider() {
    let (refused, needed) = refusal(ControllerThroughATransient).await;
    assert!(refused.contains("ReachesController"), "{refused}");
    assert!(needed.ends_with("::PerCall"), "{needed}");
}

#[module(providers: [PerCall, Between])]
struct TransientOverPerCall;

#[tokio::test]
async fn a_transient_reaching_one_resolves_in_an_execution_and_is_refused_outside() {
    let app = UloFactory::create(TransientOverPerCall).await.unwrap();

    app.resolve::<Between>(&Execution::standalone())
        .await
        .expect("an execution builds what the transient reaches");
    match app.get::<Between>().await {
        Err(ResolutionError::ExecutionRequired { token }) => {
            assert!(token.ends_with("::PerCall"), "{token}")
        }
        Ok(_) => panic!("outside an execution the transient cannot be built"),
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[injectable(scope = "execution")]
pub struct AlsoPerCall {
    #[inject]
    per_call: Arc<PerCall>,
}

#[module(providers: [PerCall, AlsoPerCall])]
struct PerCallOverPerCall;

#[tokio::test]
async fn an_execution_scoped_provider_injects_another() {
    let app = UloFactory::create(PerCallOverPerCall).await.unwrap();
    app.resolve::<AlsoPerCall>(&Execution::standalone())
        .await
        .expect("both are built in the one execution");
}

#[controller("/per-call-reaches", scope = "execution")]
pub struct PerCallReaches {
    #[inject]
    between: Between,
}

#[routes]
impl PerCallReaches {
    #[get("/")]
    fn show(&self) -> Body {
        let _ = &self.between;
        Body::text("built")
    }
}

#[module(controllers: [PerCallReaches], providers: [PerCall, Between])]
struct PerCallControllerThroughATransient;

#[tokio::test]
async fn a_controller_built_per_call_reaches_one_through_a_transient() {
    let server = TestServer::start(PerCallControllerThroughATransient).await;
    let response = server
        .client()
        .get(server.url("/per-call-reaches"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
}

#[injectable(scope = "execution")]
pub struct ConstructedPerCall {
    between: Between,
}

impl ConstructedPerCall {
    #[new]
    fn new(between: Between) -> Self {
        Self { between }
    }
}

#[module(providers: [PerCall, Between, ConstructedPerCall])]
struct ConstructorThroughATransient;

#[tokio::test]
async fn a_constructor_built_per_call_reaches_one_through_a_transient() {
    let app = UloFactory::create(ConstructorThroughATransient)
        .await
        .unwrap();
    app.resolve::<ConstructedPerCall>(&Execution::standalone())
        .await
        .expect("the transient parameter is built in the same execution");
}

#[injectable(scope = "execution")]
pub struct ReachingGuard {
    #[inject]
    between: Between,
}

#[async_trait]
impl Guard<HttpContext> for ReachingGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        let _ = &self.between;
        true
    }
}

#[controller("/guarded")]
pub struct Guarded {}

#[routes]
#[use_guards(ReachingGuard)]
impl Guarded {
    #[get("/")]
    fn show(&self) -> Body {
        Body::text("admitted")
    }
}

#[module(controllers: [Guarded], providers: [PerCall, Between, ReachingGuard])]
struct GuardThroughATransient;

#[tokio::test]
async fn a_guard_built_per_call_reaches_one_through_a_transient() {
    let server = TestServer::start(GuardThroughATransient).await;
    let response = server
        .client()
        .get(server.url("/guarded"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
}
