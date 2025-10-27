use serial_test::serial;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use toni::{
    controller, controller_struct, get, module, provider_struct, toni_factory::ToniFactory,
    Body as ToniBody, HttpAdapter, HttpRequest,
};
use toni_axum::AxumAdapter;

// ======================
// Test 1: Singleton Controller with Singleton Provider (OK - No Warning)
// ======================

#[provider_struct(pub struct SingletonProvider {})]
impl SingletonProvider {
    fn get_data(&self) -> String {
        "Singleton data".to_string()
    }
}

#[controller_struct(pub struct OkController { provider: SingletonProvider })]
#[controller("/ok")]
impl OkController {
    #[get("/test")]
    fn test(&self, _req: HttpRequest) -> ToniBody {
        ToniBody::Text(self.provider.get_data())
    }
}

// ======================
// Test 2: Singleton Controller with Request Provider (WARNING - Scope Mismatch!)
// ======================

static REQUEST_COUNTER: AtomicU32 = AtomicU32::new(0);

#[provider_struct(scope = "request", pub struct RequestScopedProvider {})]
impl RequestScopedProvider {
    fn get_request_id(&self) -> u32 {
        REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst)
    }
}

// This should trigger a warning! Singleton controller with Request-scoped dependency
#[controller_struct(pub struct ProblematicController { provider: RequestScopedProvider })]
#[controller("/problematic")]
impl ProblematicController {
    #[get("/test")]
    fn test(&self, _req: HttpRequest) -> ToniBody {
        ToniBody::Text(format!("Request ID: {}", self.provider.get_request_id()))
    }
}

// ======================
// Test 3: Request Controller with Request Provider (OK - No Warning)
// ======================

#[provider_struct(scope = "request", pub struct AnotherRequestProvider {})]
impl AnotherRequestProvider {
    fn get_data(&self) -> String {
        "Request data".to_string()
    }
}

#[controller_struct(scope = "request", pub struct CorrectController { provider: AnotherRequestProvider })]
#[controller("/correct")]
impl CorrectController {
    #[get("/test")]
    fn test(&self, _req: HttpRequest) -> ToniBody {
        ToniBody::Text(self.provider.get_data())
    }
}

// ======================
// Test 4: Mixed Dependencies (Singleton + Request) - WARNING
// ======================

#[provider_struct(pub struct CacheProvider {})]
impl CacheProvider {
    fn get_cached(&self) -> String {
        "Cached".to_string()
    }
}

#[provider_struct(scope = "request", pub struct SessionProvider {})]
impl SessionProvider {
    fn get_session(&self) -> String {
        "Session".to_string()
    }
}

// This should trigger a warning for SessionProvider being Request-scoped
#[controller_struct(pub struct MixedController {
    cache: CacheProvider,
    session: SessionProvider,
})]
#[controller("/mixed")]
impl MixedController {
    #[get("/test")]
    fn test(&self, _req: HttpRequest) -> ToniBody {
        ToniBody::Text(format!(
            "{} + {}",
            self.cache.get_cached(),
            self.session.get_session()
        ))
    }
}

// ======================
// Modules
// ======================

#[module(
    providers: [SingletonProvider],
    controllers: [OkController],
)]
impl OkModule {}

#[module(
    providers: [RequestScopedProvider],
    controllers: [ProblematicController],
)]
impl ProblematicModule {}

#[module(
    providers: [AnotherRequestProvider],
    controllers: [CorrectController],
)]
impl CorrectModule {}

#[module(
    providers: [CacheProvider, SessionProvider],
    controllers: [MixedController],
)]
impl MixedModule {}

// ======================
// Tests
// ======================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_ok_singleton_controller_with_singleton_provider() {
        println!("\n=== Test 1: Singleton Controller with Singleton Provider ===");
        println!("Expected: No warnings");

        let port = 38090;
        let local = tokio::task::LocalSet::new();

        // Spawn server in background
        local.spawn_local(async move {
            let adapter = AxumAdapter::new();
            let factory = ToniFactory::new();
            let app = factory.create(OkModule::module_definition(), adapter).await;
            let _ = app.listen(port, "127.0.0.1").await;
        });

        // Run tests within the LocalSet
        local
            .run_until(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;

                let client = reqwest::Client::new();
                let response = client
                    .get(format!("http://127.0.0.1:{}/ok/test", port))
                    .send()
                    .await
                    .unwrap();

                assert_eq!(response.status(), 200);
                let body = response.text().await.unwrap();
                assert_eq!(body, "Singleton data");

                println!("✅ Test passed - no warnings expected\n");
            })
            .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_warning_singleton_controller_with_request_provider() {
        println!("\n=== Test 2: Singleton Controller with Request Provider ===");
        println!("Expected: ⚠️  WARNING about scope mismatch");

        let port = 38091;
        let local = tokio::task::LocalSet::new();

        // Spawn server in background - THIS SHOULD PRINT A WARNING
        local.spawn_local(async move {
            let adapter = AxumAdapter::new();
            let factory = ToniFactory::new();
            let app = factory
                .create(ProblematicModule::module_definition(), adapter)
                .await;
            let _ = app.listen(port, "127.0.0.1").await;
        });

        // Run tests within the LocalSet
        local
            .run_until(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;

                println!("⚠️  Check console output above - you should see a warning about ProblematicController");
                println!("    having a Request-scoped dependency (RequestScopedProvider)\n");
            })
            .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_ok_request_controller_with_request_provider() {
        println!("\n=== Test 3: Request Controller with Request Provider ===");
        println!("Expected: No warnings");

        let port = 38092;
        let local = tokio::task::LocalSet::new();

        // Spawn server in background
        local.spawn_local(async move {
            let adapter = AxumAdapter::new();
            let factory = ToniFactory::new();
            let app = factory
                .create(CorrectModule::module_definition(), adapter)
                .await;
            let _ = app.listen(port, "127.0.0.1").await;
        });

        // Run tests within the LocalSet
        local
            .run_until(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;

                let client = reqwest::Client::new();
                let response = client
                    .get(format!("http://127.0.0.1:{}/correct/test", port))
                    .send()
                    .await
                    .unwrap();

                assert_eq!(response.status(), 200);
                let body = response.text().await.unwrap();
                assert_eq!(body, "Request data");

                println!("✅ Test passed - no warnings expected\n");
            })
            .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_warning_mixed_dependencies() {
        println!("\n=== Test 4: Mixed Dependencies (Singleton + Request) ===");
        println!("Expected: ⚠️  WARNING about SessionProvider being Request-scoped");

        let port = 38093;
        let local = tokio::task::LocalSet::new();

        // Spawn server in background - THIS SHOULD PRINT A WARNING
        local.spawn_local(async move {
            let adapter = AxumAdapter::new();
            let factory = ToniFactory::new();
            let app = factory
                .create(MixedModule::module_definition(), adapter)
                .await;
            let _ = app.listen(port, "127.0.0.1").await;
        });

        // Run tests within the LocalSet
        local
            .run_until(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;

                println!("⚠️  Check console output above - you should see a warning about MixedController");
                println!("    having a Request-scoped dependency (SessionProvider)\n");
            })
            .await;
    }
}
