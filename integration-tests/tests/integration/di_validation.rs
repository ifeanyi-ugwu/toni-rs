use toni::{module, provider, toni_factory::ToniFactory};

#[tokio::test]
async fn valid_singleton_injects_singleton() {
    #[provider]
    pub struct ServiceA {}
    impl ServiceA {}

    #[provider]
    pub struct ServiceB {
        #[inject]
        dep: ServiceA,
    }
    impl ServiceB {}

    #[module(providers: [ServiceA, ServiceB])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    app.get::<ServiceB>()
        .await
        .expect("ServiceB with ServiceA dep should resolve");
}

#[tokio::test]
async fn valid_request_injects_singleton() {
    #[provider]
    pub struct SingletonService {}
    impl SingletonService {}

    #[provider(scope = "request")]
    pub struct RequestService {
        #[inject]
        dep: SingletonService,
    }
    impl RequestService {}

    #[module(providers: [SingletonService, RequestService])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    app.resolve::<RequestService>(&parts)
        .await
        .expect("request-scoped service with singleton dep should resolve");
}

#[tokio::test]
async fn valid_transient_injects_any_scope() {
    #[provider]
    pub struct SingletonService {}
    impl SingletonService {}

    #[provider(scope = "request")]
    pub struct RequestService {}
    impl RequestService {}

    #[provider(scope = "transient")]
    pub struct TransientService {
        #[inject]
        singleton: SingletonService,
        #[inject]
        request: RequestService,
    }
    impl TransientService {}

    #[module(providers: [SingletonService, RequestService, TransientService])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    app.resolve::<TransientService>(&parts)
        .await
        .expect("transient with mixed deps should resolve");
}

#[tokio::test]
#[should_panic(expected = "Scope validation error")]
async fn singleton_cannot_inject_request_scoped() {
    #[provider(scope = "request")]
    pub struct RequestService {}
    impl RequestService {}

    #[provider]
    pub struct SingletonService {
        #[inject]
        request_dep: RequestService,
    }
    impl SingletonService {}

    #[module(providers: [RequestService, SingletonService])]
    impl InvalidModule {}

    let _app = ToniFactory::create(InvalidModule::module_definition()).await;
}

#[tokio::test]
async fn singleton_can_inject_transient() {
    #[provider(scope = "transient")]
    pub struct TransientService {}
    impl TransientService {}

    #[provider]
    pub struct SingletonService {
        #[inject]
        transient_dep: TransientService,
    }
    impl SingletonService {}

    #[module(providers: [TransientService, SingletonService])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    app.get::<SingletonService>()
        .await
        .expect("singleton with transient dep should resolve");
}

#[tokio::test]
async fn request_can_inject_transient() {
    #[provider(scope = "transient")]
    pub struct TransientService {}
    impl TransientService {}

    #[provider(scope = "request")]
    pub struct RequestService {
        #[inject]
        transient_dep: TransientService,
    }
    impl RequestService {}

    #[module(providers: [TransientService, RequestService])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    app.resolve::<RequestService>(&parts)
        .await
        .expect("request-scoped with transient dep should resolve");
}

#[tokio::test]
async fn complex_valid_hierarchy() {
    #[provider]
    pub struct BaseService {}
    impl BaseService {}

    #[provider]
    pub struct MiddleService {
        #[inject]
        base: BaseService,
    }
    impl MiddleService {}

    #[provider(scope = "request")]
    pub struct TopService {
        #[inject]
        middle: MiddleService,
        #[inject]
        base: BaseService,
    }
    impl TopService {}

    #[module(providers: [BaseService, MiddleService, TopService])]
    impl TestModule {}

    let app = ToniFactory::create(TestModule::module_definition()).await;
    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    app.resolve::<TopService>(&parts)
        .await
        .expect("three-level hierarchy should resolve");
}

#[tokio::test]
#[should_panic(expected = "Scope validation error")]
async fn explicit_singleton_with_request_fails() {
    #[provider(scope = "request")]
    pub struct RequestService {}
    impl RequestService {}

    #[provider(scope = "singleton")]
    pub struct ExplicitSingleton {
        #[inject]
        request_dep: RequestService,
    }
    impl ExplicitSingleton {}

    #[module(providers: [RequestService, ExplicitSingleton])]
    impl TestModule {}

    let _app = ToniFactory::create(TestModule::module_definition()).await;
}
