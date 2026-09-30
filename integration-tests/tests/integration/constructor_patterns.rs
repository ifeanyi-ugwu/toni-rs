//! `#[new]` on a provider, on a controller, and taking parameters the struct
//! never stores.
//!
//! A bare `new()` is not detected — construction is field injection unless an
//! attribute says otherwise — so each shape is a distinct claim about what the
//! macro emits. The parameter case is the one field injection cannot express.
use crate::common::TestServer;
use std::sync::Arc;
use std::time::Duration;
use ulo::http::Body;
use ulo::{controller, get, injectable, module, new, provide, routes};
#[tokio::test]
async fn provider_constructor_patterns() {
    #[injectable]
    pub struct BaseService {
        value: String,
    }
    impl BaseService {
        #[new]
        pub fn new() -> Self {
            Self {
                value: "base".to_string(),
            }
        }

        pub fn get_value(&self) -> String {
            self.value.clone()
        }
    }

    #[injectable]
    pub struct AutoNewService {
        base_value: String,
    }
    impl AutoNewService {
        #[new]
        pub fn new(base: Arc<BaseService>) -> Self {
            Self {
                base_value: base.get_value(),
            }
        }

        pub fn get_value(&self) -> String {
            self.base_value.clone()
        }
    }

    #[injectable]
    pub struct CustomInitService {
        combined: String,
    }
    impl CustomInitService {
        #[new]
        fn create(base: Arc<BaseService>) -> Self {
            Self {
                combined: format!("custom:{}", base.get_value()),
            }
        }

        pub fn get_value(&self) -> String {
            self.combined.clone()
        }
    }

    #[injectable]
    pub struct ComplexInitService {
        base_value: String,
        settings: Vec<String>,
        timeout: Duration,
    }
    impl ComplexInitService {
        #[new]
        fn build(base: Arc<BaseService>) -> Self {
            let mut settings = Vec::new();
            settings.push(format!("setting:{}", base.get_value()));
            settings.push("s2".to_string());

            Self {
                base_value: base.get_value(),
                settings,
                timeout: Duration::from_secs(60),
            }
        }

        pub fn get_settings_count(&self) -> usize {
            self.settings.len()
        }
    }

    #[injectable]
    pub struct DefaultFallbackService {
        name: String,
        count: i32,
    }

    impl DefaultFallbackService {
        pub fn get_info(&self) -> String {
            format!("name='{}',count={}", self.name, self.count)
        }
    }

    #[controller("/providers")]
    pub struct ProviderTestController {
        #[inject]
        base: Arc<BaseService>,
        #[inject]
        auto: Arc<AutoNewService>,
        #[inject]
        custom: Arc<CustomInitService>,
        #[inject]
        complex: Arc<ComplexInitService>,
        #[inject]
        fallback: Arc<DefaultFallbackService>,
    }

    #[routes]
    impl ProviderTestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(format!(
                "{}|{}|{}|{}|{}",
                self.base.get_value(),
                self.auto.get_value(),
                self.custom.get_value(),
                self.complex.get_settings_count(),
                self.fallback.get_info()
            ))
        }
    }

    #[module(
        providers: [
            BaseService,
            AutoNewService,
            CustomInitService,
            ComplexInitService,
            DefaultFallbackService,
        ],
        controllers: [ProviderTestController]
    )]
    impl ProviderPatternsModule {}

    let server = TestServer::start(ProviderPatternsModule).await;
    let resp = server
        .client()
        .get(server.url("/providers/test"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, "base|base|custom:base|2|name='',count=0");
}

#[tokio::test]
async fn controller_constructor_patterns() {
    #[injectable]
    pub struct DataService {
        value: String,
    }
    impl DataService {
        #[new]
        pub fn new() -> Self {
            Self {
                value: "data".to_string(),
            }
        }

        pub fn get_value(&self) -> String {
            self.value.clone()
        }
    }

    #[controller("/auto")]
    pub struct AutoNewController {
        data_value: String,
    }

    #[routes]
    impl AutoNewController {
        #[new]
        pub fn new(data: Arc<DataService>) -> Self {
            Self {
                data_value: data.get_value(),
            }
        }

        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(format!("auto: {}", self.data_value))
        }
    }

    #[controller("/custom")]
    pub struct CustomInitController {
        combined: String,
    }

    #[routes]
    impl CustomInitController {
        #[new]
        fn create(data: Arc<DataService>) -> Self {
            Self {
                combined: format!("custom: {}", data.get_value()),
            }
        }

        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.combined.clone())
        }
    }

    #[controller("/default")]
    pub struct DefaultFallbackController {
        name: String,
        count: i32,
    }

    #[routes]
    impl DefaultFallbackController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(format!("name='{}', count={}", self.name, self.count))
        }
    }

    #[module(
        providers: [DataService],
        controllers: [AutoNewController, CustomInitController, DefaultFallbackController]
    )]
    impl ControllerPatternsModule {}

    let server = TestServer::start(ControllerPatternsModule).await;

    let resp = server
        .client()
        .get(server.url("/auto/test"))
        .send()
        .await
        .unwrap();
    assert!(resp.text().await.unwrap().contains("data"));

    let resp = server
        .client()
        .get(server.url("/custom/test"))
        .send()
        .await
        .unwrap();
    assert!(resp.text().await.unwrap().contains("custom"));

    let resp = server
        .client()
        .get(server.url("/default/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "name='', count=0");
}

#[tokio::test]
async fn constructor_param_injection_patterns() {
    ulo::key!(CustomDatabase: DatabaseService);

    #[injectable]
    pub struct DatabaseService {}
    impl DatabaseService {
        pub fn query(&self) -> String {
            "data".to_string()
        }
    }

    #[injectable]
    pub struct ConfigService {}
    impl ConfigService {
        pub fn get_config(&self) -> String {
            "config".to_string()
        }
    }

    #[injectable]
    pub struct CacheService {}
    impl CacheService {
        pub fn get_cache(&self) -> String {
            "cache".to_string()
        }
    }

    // #[inject] on params is redundant but supported for explicitness
    #[injectable]
    pub struct BasicParamService {}
    impl BasicParamService {
        #[new]
        fn new(#[inject] _db: Arc<DatabaseService>) -> Self {
            Self {}
        }

        pub fn get_data(&self, db: &DatabaseService) -> String {
            format!("basic:{}", db.query())
        }
    }

    #[injectable]
    pub struct TokenParamService {}
    impl TokenParamService {
        #[new]
        fn new(#[inject(CustomDatabase)] _db: Arc<DatabaseService>) -> Self {
            Self {}
        }

        pub fn get_data(&self, db: &DatabaseService) -> String {
            format!("token:{}", db.query())
        }
    }

    #[injectable]
    pub struct MixedParamService {}
    impl MixedParamService {
        #[new]
        fn new(
            #[inject] _config: Arc<ConfigService>,
            _cache: Arc<CacheService>, // No #[inject], still works via type token
        ) -> Self {
            Self {}
        }

        pub fn get_info(&self, config: &ConfigService, cache: &CacheService) -> String {
            format!("{}-{}", config.get_config(), cache.get_cache())
        }
    }

    #[controller("/basic")]
    pub struct BasicParamController {
        #[inject]
        db: Arc<DatabaseService>,
        #[inject]
        service: Arc<BasicParamService>,
    }

    #[routes]
    impl BasicParamController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.service.get_data(&self.db))
        }
    }

    #[controller("/token")]
    pub struct TokenParamController {
        #[inject(CustomDatabase)]
        db: Arc<DatabaseService>,
        #[inject]
        service: Arc<TokenParamService>,
    }

    #[routes]
    impl TokenParamController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.service.get_data(&self.db))
        }
    }

    #[controller("/mixed")]
    pub struct MixedParamController {
        #[inject]
        config: Arc<ConfigService>,
        #[inject]
        cache: Arc<CacheService>,
        #[inject]
        service: Arc<MixedParamService>,
    }

    #[routes]
    impl MixedParamController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.service.get_info(&self.config, &self.cache))
        }
    }

    #[module(
        providers: [
            DatabaseService,
            provide!(CustomDatabase => DatabaseService),
            ConfigService,
            CacheService,
            BasicParamService,
            TokenParamService,
            MixedParamService,
        ],
        controllers: [BasicParamController, TokenParamController, MixedParamController]
    )]
    impl ParamInjectionModule {}

    let server = TestServer::start(ParamInjectionModule).await;

    let resp = server
        .client()
        .get(server.url("/basic/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "basic:data");

    let resp = server
        .client()
        .get(server.url("/token/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "token:data");

    let resp = server
        .client()
        .get(server.url("/mixed/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "config-cache");
}
