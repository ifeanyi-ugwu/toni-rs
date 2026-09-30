//! The forms a module may be declared in — attribute and impl, nested imports,
//! selective exports — and what each makes visible.
//!
//! Exports decide the visibility boundary, so the claim worth pinning is
//! negative as much as positive: a provider not exported must not resolve from
//! an importing module, and a nested import must not flatten the tree.
use crate::common::TestServer;
use std::sync::Arc;
use ulo::di::ModuleRef;
use ulo::http::Body;
use ulo::{controller, get, injectable, module, routes};
#[tokio::test]
async fn global_modules_attribute_syntax() {
    #[injectable]
    pub struct GlobalService {}
    impl GlobalService {
        pub fn message(&self) -> String {
            "global".to_string()
        }
    }

    #[module(
        global: true,
        providers: [GlobalService],
        exports: [GlobalService],
    )]
    impl GlobalModule {}

    #[injectable]
    pub struct LocalService {
        #[inject]
        global: Arc<GlobalService>,
    }
    impl LocalService {
        pub fn get_message(&self) -> String {
            self.global.message()
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        service: Arc<LocalService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.service.get_message())
        }
    }

    #[module(
        imports: [GlobalModule],
        providers: [LocalService],
        controllers: [TestController],
    )]
    impl AppModule {}

    let server = TestServer::start(AppModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "global");
}

#[tokio::test]
async fn module_ref_runtime_provider_access() {
    #[injectable]
    pub struct RuntimeService {}
    impl RuntimeService {
        pub fn value(&self) -> i32 {
            42
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        module_ref: ModuleRef,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        async fn test(&self) -> Body {
            let service = self.module_ref.get::<RuntimeService>().await;
            Body::text(format!("{}", service.unwrap().value()))
        }
    }

    #[module(
        providers: [RuntimeService],
        controllers: [TestController],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "42");
}

#[tokio::test]
async fn nested_module_imports() {
    #[injectable]
    pub struct DatabaseService {}
    impl DatabaseService {
        pub fn query(&self) -> String {
            "data".to_string()
        }
    }

    #[module(
        providers: [DatabaseService],
        exports: [DatabaseService],
    )]
    impl DatabaseModule {}

    #[injectable]
    pub struct FeatureService {
        #[inject]
        db: Arc<DatabaseService>,
    }
    impl FeatureService {
        pub fn get_data(&self) -> String {
            self.db.query()
        }
    }

    #[module(
        imports: [DatabaseModule],
        providers: [FeatureService],
        exports: [FeatureService],
    )]
    impl FeatureModule {}

    #[controller("")]
    pub struct TestController {
        #[inject]
        feature: Arc<FeatureService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.feature.get_data())
        }
    }

    #[module(
        imports: [FeatureModule],
        controllers: [TestController],
    )]
    impl AppModule {}

    let server = TestServer::start(AppModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "data");
}

#[tokio::test]
async fn module_exports_selective_providers() {
    #[injectable]
    pub struct PublicService {}
    impl PublicService {
        pub fn data(&self) -> String {
            "public".to_string()
        }
    }

    #[injectable]
    pub struct PrivateService {}
    impl PrivateService {}

    #[module(
        providers: [PublicService, PrivateService],
        exports: [PublicService],
    )]
    impl SourceModule {}

    #[injectable]
    pub struct ConsumerService {
        #[inject]
        public: Arc<PublicService>,
    }
    impl ConsumerService {
        pub fn get_data(&self) -> String {
            self.public.data()
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        consumer: Arc<ConsumerService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.consumer.get_data())
        }
    }

    #[module(
        imports: [SourceModule],
        providers: [ConsumerService],
        controllers: [TestController],
    )]
    impl AppModule {}

    let server = TestServer::start(AppModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "public");
}

#[tokio::test]
async fn module_struct_syntax() {
    #[injectable]
    pub struct TestService {}
    impl TestService {
        pub fn message(&self) -> String {
            "struct-syntax".to_string()
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        service: Arc<TestService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.service.message())
        }
    }

    #[module(
        providers: [TestService],
        controllers: [TestController],
    )]
    pub struct TestModule;

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "struct-syntax");
}
