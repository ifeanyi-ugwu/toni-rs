//! `provide!` registers a provider under a marker, reading the form from the
//! source's syntax and taking it from a keyword when told.
//!
//! Reading the syntax is the part that can be wrong without being loud: a value
//! read as a factory, or the reverse, still registers a provider and still
//! resolves. Each form — an inline value, an inline closure, a bare type name,
//! `alias`, `value`, `factory` — is asserted against what it produced, not
//! merely that it produced something.
use crate::common::TestServer;
use std::time::Duration;
use ulo::http::Body;
use ulo::{controller, get, injectable, module, new, provide, routes};
#[injectable]
pub struct ConfigService {
    env: String,
}
impl ConfigService {
    #[new]
    pub fn new() -> Self {
        Self {
            env: "prod".to_string(),
        }
    }
}

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
}

#[injectable]
pub struct CacheService {
    url: String,
}
impl CacheService {
    #[new]
    pub fn new() -> Self {
        Self {
            url: "redis://localhost".to_string(),
        }
    }
}

#[tokio::test]
async fn provide_macro_patterns() {
    ulo::key!(ApiKey: String);
    ulo::key!(Port: u16);
    ulo::key!(Timeout: Duration);
    ulo::key!(MaxConnections: i32);
    ulo::key!(Logger: String);
    ulo::key!(PrimaryDb: DatabaseService);
    ulo::key!(CacheAlias: CacheService);
    ulo::key!(ExplicitValue: String);
    ulo::key!(ExplicitFactory: String);
    #[injectable]
    pub struct AppService {
        #[inject(ApiKey)]
        api_key: String,

        #[inject(Port)]
        port: u16,

        #[inject(Timeout)]
        timeout: Duration,

        #[inject(MaxConnections)]
        max_connections: i32,

        #[inject(Logger)]
        logger: String,

        #[inject(PrimaryDb)]
        database: DatabaseService,

        #[inject(CacheAlias)]
        cache: CacheService,

        #[inject(ExplicitValue)]
        explicit_value: String,

        #[inject(ExplicitFactory)]
        explicit_factory: String,
    }
    impl AppService {
        pub fn get_info(&self) -> String {
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                self.api_key,
                self.port,
                self.timeout.as_secs(),
                self.max_connections,
                self.logger,
                self.database.host,
                self.cache.url,
                self.explicit_value,
                self.explicit_factory
            )
        }
    }

    #[controller("/app")]
    pub struct AppController {
        #[inject]
        app: AppService,
    }

    #[routes]
    impl AppController {
        #[get("/info")]
        fn info(&self) -> Body {
            Body::text(self.app.get_info())
        }
    }

    #[module(
        providers: [
            ConfigService,
            DatabaseService,
            CacheService,

            // An inline expression is a value
            provide!(ApiKey => "secret_key".to_string()),
            provide!(Port => 8080_u16),
            provide!(Timeout => Duration::from_secs(30)),

            // An inline closure is a factory
            provide!(MaxConnections => async || 100_i32),
            provide!(Logger => async |config: ConfigService| {
                format!("logger:{}", config.env)
            }),

            // A bare name is the type's own declaration, here under a key
            provide!(PrimaryDb => DatabaseService),

            // `alias` is a second name for an existing binding
            provide!(CacheAlias => alias CacheService),

            // `value` and `factory` name the form where the syntax would too
            provide!(ExplicitValue => value "explicit".to_string()),
            provide!(ExplicitFactory => factory async || "factory_result".to_string()),

            AppService,
        ],
        controllers: [AppController]
    )]
    impl UnifiedProvideModule {}

    let server = TestServer::start(UnifiedProvideModule).await;
    let resp = server
        .client()
        .get(server.url("/app/info"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.text().await.unwrap(),
        "secret_key|8080|30|100|logger:prod|localhost:5432|redis://localhost|explicit|factory_result"
    );
}
