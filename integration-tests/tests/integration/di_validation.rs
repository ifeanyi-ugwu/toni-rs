//! Which scope may inject which: a singleton cannot depend on an execution-scoped
//! provider, and anything may depend on a transient one.
//!
//! A singleton outlives every request, so holding something execution-scoped
//! means holding one arbitrary request's state forever. That failure is
//! invisible at runtime — the application serves correctly until two requests
//! disagree — so `create` refuses it, naming the provider built at startup and
//! the execution-scoped one it reaches, through however many transients.
use ulo::di::{Execution, ModuleMetadata, ResolutionError};
use ulo::enhancer::Guard;
use ulo::http::{Body, HttpContext};
use ulo::{
    StartupError, UloFactory, async_trait, controller, get, injectable, module, new, routes,
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
        dep: ServiceA,
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
        dep: SingletonService,
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
        singleton: SingletonService,
        #[inject]
        request: RequestService,
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
        request_dep: RequestService,
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
        base: BaseService,
    }
    impl MiddleService {}

    #[injectable(scope = "execution")]
    pub struct TopService {
        #[inject]
        middle: MiddleService,
        #[inject]
        base: BaseService,
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
        request_dep: RequestService,
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
    per_call: PerCall,
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
