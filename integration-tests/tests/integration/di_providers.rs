//! `provide!`'s four forms under a key — a value, a factory, an alias and a type
//! — each resolving to what it registered.
//!
//! They are four ways to reach one provider store, and a consumer picks between
//! them by what it has to hand — a constant, a closure, an existing binding, a
//! type's own declaration. Coverage of one says nothing about the others. The
//! final test runs them in a single module, where a key collision between two
//! forms would surface.
use crate::common::TestServer;
use std::sync::Arc;
use std::time::Duration;
use ulo::http::Body;
use ulo::{controller, get, injectable, module, new, provide, routes};
#[tokio::test]
async fn value_injects_a_constant() {
    ulo::key!(Port: u16);
    #[controller()]
    pub struct TestController {
        #[inject(Port)]
        port: Arc<u16>,
    }

    #[routes]
    impl TestController {
        #[get("/port")]
        fn port(&self) -> Body {
            Body::text(self.port.to_string())
        }
    }

    #[module(
        providers: [provide!(Port => 3000_u16)],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/port"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "3000");
}

#[tokio::test]
async fn factory_without_dependencies() {
    ulo::key!(RequestId: String);
    use std::sync::atomic::{AtomicU32, Ordering};

    static CALL_COUNT: AtomicU32 = AtomicU32::new(0);

    #[controller("")]
    pub struct TestController {
        #[inject(RequestId)]
        request_id: Arc<String>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.request_id.to_string())
        }
    }

    #[module(
        providers: [provide!(RequestId => async || {
            CALL_COUNT.fetch_add(1, Ordering::SeqCst);
            format!("req_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis())
        })],
        controllers: [TestController]
    )]
    impl TestModule {}

    CALL_COUNT.store(0, Ordering::SeqCst);
    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert!(resp.text().await.unwrap().starts_with("req_"));
    assert_eq!(
        CALL_COUNT.load(Ordering::SeqCst),
        1,
        "a singleton factory runs once"
    );
}

#[tokio::test]
async fn factory_with_a_dependency() {
    ulo::key!(AppInfo: String);
    #[injectable]
    pub struct ConfigService {
        env: String,
    }
    impl ConfigService {
        #[new]
        pub fn new() -> Self {
            Self {
                env: "production".to_string(),
            }
        }

        pub fn get_env(&self) -> String {
            self.env.clone()
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject(AppInfo)]
        value: Arc<String>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.value.to_string())
        }
    }

    #[module(
        providers: [
            ConfigService,
            provide!(AppInfo => async |config: Arc<ConfigService>| {
                format!("App running in {} mode", config.get_env())
            })
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "App running in production mode");
}

#[tokio::test]
async fn factory_awaiting_in_its_body() {
    ulo::key!(AsyncStatus: String);
    #[injectable]
    pub struct LoggerService {
        level: String,
    }
    impl LoggerService {
        #[new]
        pub fn new() -> Self {
            Self {
                level: "info".to_string(),
            }
        }

        pub fn log(&self, msg: &str) -> String {
            format!("[{}] {}", self.level, msg)
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject(AsyncStatus)]
        value: Arc<String>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.value.to_string())
        }
    }

    #[module(
        providers: [
            LoggerService,
            provide!(AsyncStatus => async |logger: Arc<LoggerService>| {
                tokio::time::sleep(Duration::from_millis(1)).await;
                logger.log("System initialized")
            })
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "[info] System initialized");
}

#[tokio::test]
async fn alias_creates_a_second_key() {
    ulo::key!(Config: ConfigService);
    #[injectable]
    pub struct ConfigService {
        env: String,
    }
    impl ConfigService {
        #[new]
        pub fn new() -> Self {
            Self {
                env: "production".to_string(),
            }
        }
        pub fn get_env(&self) -> &str {
            &self.env
        }
    }

    // Injects ConfigService twice: once by type, once through the "Config" alias.
    // If the alias registration doesn't create a working resolution path,
    // DI startup panics and the test never reaches the HTTP assertion.
    #[injectable]
    pub struct VerifyService {
        #[inject]
        by_type: Arc<ConfigService>,
        #[inject(Config)]
        by_alias: Arc<ConfigService>,
    }
    impl VerifyService {
        pub fn report(&self) -> String {
            format!("{}|{}", self.by_type.get_env(), self.by_alias.get_env())
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        verify: Arc<VerifyService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.verify.report())
        }
    }

    #[module(
        providers: [
            ConfigService,
            provide!(Config => alias ConfigService),
            VerifyService,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "production|production");
}

#[tokio::test]
async fn type_under_a_marker() {
    ulo::key!(PrimaryDb: DatabaseService);
    #[injectable]
    pub struct DatabaseService {
        host: String,
    }
    impl DatabaseService {
        #[new]
        pub fn new() -> Self {
            Self {
                host: "localhost:5432".to_string(),
            }
        }
        pub fn get_host(&self) -> &str {
            &self.host
        }
    }

    // Injects DatabaseService through the `PrimaryDb` marker.
    // If the rebinding doesn't wire the resolution path, startup panics.
    #[injectable]
    pub struct AppService {
        #[inject(PrimaryDb)]
        primary: Arc<DatabaseService>,
    }
    impl AppService {
        pub fn get_info(&self) -> String {
            self.primary.get_host().to_string()
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        app: Arc<AppService>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.app.get_info())
        }
    }

    #[module(
        providers: [
            DatabaseService,
            provide!(PrimaryDb => DatabaseService),
            AppService,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "localhost:5432");
}

#[tokio::test]
async fn every_form_in_one_module() {
    ulo::key!(Timeout: Duration);
    ulo::key!(AppName: String);
    ulo::key!(RequestId: String);
    ulo::key!(Port: u16);
    ulo::key!(Logger: LoggerService);
    ulo::key!(AsyncStatus: String);
    ulo::key!(PrimaryConfig: ConfigService);
    ulo::key!(AppInfo: String);
    ulo::key!(SecondaryLogger: LoggerService);
    ulo::key!(AppPort: u16);
    ulo::key!(Config: ConfigService);
    #[injectable]
    pub struct ConfigService {
        env: String,
    }
    impl ConfigService {
        #[new]
        pub fn new() -> Self {
            Self {
                env: "production".to_string(),
            }
        }

        pub fn get_env(&self) -> String {
            self.env.clone()
        }
    }

    #[injectable]
    pub struct LoggerService {
        level: String,
    }
    impl LoggerService {
        #[new]
        pub fn new() -> Self {
            Self {
                level: "info".to_string(),
            }
        }

        pub fn log(&self, msg: &str) -> String {
            format!("[{}] {}", self.level, msg)
        }
    }

    // Consumes one alias and one marker to prove they're injectable alongside
    // value/factory providers in the same module.
    #[injectable]
    pub struct AliasMarkerConsumer {
        #[inject(Config)]
        config_via_alias: Arc<ConfigService>,
        #[inject(PrimaryConfig)]
        config_via_marker: Arc<ConfigService>,
    }
    impl AliasMarkerConsumer {
        pub fn report(&self) -> String {
            format!(
                "{}|{}",
                self.config_via_alias.get_env(),
                self.config_via_marker.get_env()
            )
        }
    }

    #[controller("")]
    pub struct TestController {
        #[inject]
        consumer: Arc<AliasMarkerConsumer>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text(self.consumer.report())
        }
    }

    #[module(
        providers: [
            ConfigService,
            LoggerService,
            provide!(AppName => "UloApp".to_string()),
            provide!(Port => 3000_u16),
            provide!(Timeout => Duration::from_secs(30)),
            provide!(RequestId => async || {
                format!("req_{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis())
            }),
            provide!(AppInfo => async |config: Arc<ConfigService>| {
                format!("App running in {} mode", config.get_env())
            }),
            provide!(AsyncStatus => async |logger: Arc<LoggerService>| {
                tokio::time::sleep(Duration::from_millis(1)).await;
                logger.log("System initialized")
            }),
            provide!(Config => alias ConfigService),
            provide!(Logger => alias LoggerService),
            provide!(AppPort => alias Port),
            provide!(PrimaryConfig => ConfigService),
            provide!(SecondaryLogger => LoggerService),
            AliasMarkerConsumer,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "production|production");
}
