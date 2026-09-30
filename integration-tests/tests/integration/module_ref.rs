//! `ModuleRef` resolves providers at runtime, scoped to the module that handed
//! it out.
//!
//! Resolving in the module's own scope is the point: a handle that searched every module would
//! satisfy every lookup and erase the module boundary, so the refusals matter
//! more than the successes. Execution-scoped resolution through a handle
//! is covered too — it needs an execution to resolve into, and asking without
//! one is a refusal rather than a panic.
use std::sync::Arc;
use ulo::prelude::*;
use uuid::Uuid;

// Test providers
#[injectable]
pub struct DatabaseService {
    pub connection_string: String,
}

impl DatabaseService {
    #[new]
    pub fn new() -> Self {
        Self {
            connection_string: "postgres://localhost:5432".to_string(),
        }
    }
}

#[injectable]
pub struct CacheService {
    pub host: String,
}

impl CacheService {
    #[new]
    pub fn new() -> Self {
        Self {
            host: "redis://localhost:6379".to_string(),
        }
    }
}

#[derive(Debug)]
#[injectable(scope = "execution")]
pub struct RequestScopedService {
    pub id: String,
}

impl RequestScopedService {
    #[new]
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
        }
    }
}

// Test service that uses ModuleRef for dynamic resolution
#[injectable]
pub struct PluginLoader {
    #[inject]
    module_ref: ModuleRef,
}

impl PluginLoader {
    /// The current module only (the default)
    pub async fn load_service_here(&self) -> Option<Arc<DatabaseService>> {
        self.module_ref.get::<DatabaseService>().await.ok()
    }

    /// Everything visible to the module: its own, its imports' exports, the globals
    pub async fn load_service_visible(&self) -> Option<Arc<CacheService>> {
        self.module_ref.get::<CacheService>().visible().await.ok()
    }

    /// A provider of another module, which the current module alone does not hold
    pub async fn load_cache_here(&self) -> Option<Arc<CacheService>> {
        self.module_ref.get::<CacheService>().await.ok()
    }

    /// Resolution through a key, in the current module
    pub async fn load_by_key_here(&self) -> Option<Arc<DatabaseService>> {
        self.module_ref.get_key::<DatabaseService>().await.ok()
    }

    pub async fn load_request_scoped(&self) -> Result<Arc<RequestScopedService>, String> {
        self.module_ref
            .get::<RequestScopedService>()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn resolve_request_scoped(
        &self,
        execution: &Execution,
    ) -> Result<Arc<RequestScopedService>, String> {
        self.module_ref
            .resolve::<RequestScopedService>(execution)
            .await
            .map_err(|e| e.to_string())
    }

    /// Get current module name
    pub fn current_module(&self) -> String {
        self.module_ref.current_module().to_string()
    }
}

// Module 1 - contains DatabaseService and PluginLoader
#[module(
    providers: [DatabaseService, RequestScopedService, PluginLoader],
    exports: [DatabaseService, PluginLoader],
)]
impl Module1 {}

// Module 2 - contains CacheService (global module)
#[module(
    providers: [CacheService],
    exports: [CacheService],
    global: true,
)]
impl Module2 {}

// Root module - imports both modules
#[module(
    imports: [Module1, Module2],
)]
impl AppModule {}

#[tokio::test]
async fn test_module_ref_resolves_its_own_provider() {
    let app = UloFactory::create(AppModule).await.unwrap();

    // Get PluginLoader from Module1
    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // DatabaseService is the current module's own provider
    let db_service = plugin_loader
        .load_service_here()
        .await
        .expect("Should resolve DatabaseService from its own module");

    assert_eq!(db_service.connection_string, "postgres://localhost:5432");
}

#[tokio::test]
async fn test_module_ref_visible_reaches_a_global_export() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // CacheService is a global module's export, visible to every module
    let cache_service = plugin_loader
        .load_service_visible()
        .await
        .expect("Should resolve CacheService through .visible()");

    assert_eq!(cache_service.host, "redis://localhost:6379");
}

#[tokio::test]
async fn test_module_ref_misses_another_modules_provider() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // CacheService is another module's provider, so the current module alone does not hold it
    let result = plugin_loader.load_cache_here().await;

    assert!(
        result.is_none(),
        "Should fail to resolve CacheService from the current module alone (different module)"
    );
}

#[tokio::test]
async fn test_module_ref_resolves_a_type_as_its_own_key() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // A type written as a key names its own slot
    let db_service = plugin_loader
        .load_by_key_here()
        .await
        .expect("Should resolve DatabaseService by its key");

    assert_eq!(db_service.connection_string, "postgres://localhost:5432");
}

#[tokio::test]
async fn test_module_ref_current_module() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // current_module() returns the module's identity, which is its fully-qualified type name.
    let module_name = plugin_loader.current_module();
    assert_eq!(
        module_name,
        std::any::type_name::<Module1>(),
        "Should return the module's fully-qualified identity"
    );
}

#[tokio::test]
async fn test_module_ref_singleton_behavior() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader1 = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    let plugin_loader2 = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // Both should point to the same module
    assert_eq!(
        plugin_loader1.current_module(),
        plugin_loader2.current_module(),
        "ModuleRef should be singleton per module"
    );

    // Both should resolve the same service instance (since DatabaseService is singleton)
    let db1 = plugin_loader1
        .load_service_here()
        .await
        .expect("Should resolve");
    let db2 = plugin_loader2
        .load_service_here()
        .await
        .expect("Should resolve");

    assert_eq!(db1.connection_string, db2.connection_string);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_module_ref_works_from_any_thread() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    // Spawn onto a fresh OS thread — tokio's multi-threaded runtime can poll futures
    // on any worker, so ModuleRef::get() must work regardless of which thread calls it.
    let result = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(plugin_loader.load_service_here())
    })
    .join()
    .expect("thread should not panic");

    assert!(
        result.is_some(),
        "ModuleRef::get should work from any thread, not just the initialization thread"
    );
}

#[tokio::test]
async fn request_scoped_get_is_refused_not_a_panic() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    let message = plugin_loader
        .load_request_scoped()
        .await
        .expect_err("`get` has no execution to build an execution-scoped provider in");

    assert!(
        message.contains("RequestScopedService") && message.contains("execution-scoped"),
        "the refusal should name the provider and its scope, got: {message}"
    );
}

#[tokio::test]
async fn resolve_builds_a_request_scoped_provider_in_the_execution() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    let execution = Execution::standalone();

    let first = plugin_loader
        .resolve_request_scoped(&execution)
        .await
        .expect("an execution is all an execution-scoped provider needs");
    let second = plugin_loader
        .resolve_request_scoped(&execution)
        .await
        .expect("an execution is all an execution-scoped provider needs");

    assert_eq!(
        first.id, second.id,
        "one execution holds one instance of an execution-scoped provider"
    );

    let elsewhere = Execution::standalone();
    let third = plugin_loader
        .resolve_request_scoped(&elsewhere)
        .await
        .expect("an execution is all an execution-scoped provider needs");

    assert_ne!(
        first.id, third.id,
        "a second execution builds its own instance"
    );
}

/// The cache belongs to the execution, not to whoever reached it: a module's own
/// handle and the application context resolve the same instance in one execution.
#[tokio::test]
async fn the_module_and_the_application_resolve_into_one_cache() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let plugin_loader = app
        .get::<PluginLoader>()
        .await
        .expect("PluginLoader should be available");

    let execution = Execution::standalone();

    let through_module = plugin_loader
        .resolve_request_scoped(&execution)
        .await
        .expect("a module resolves in the execution it is given");
    let through_app = app
        .resolve::<RequestScopedService>(&execution)
        .await
        .expect("the application resolves in the same one");

    assert_eq!(through_module.id, through_app.id);
}
