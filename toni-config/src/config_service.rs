//! ConfigService - Injectable service for accessing configuration in providers

use crate::Config;
use std::any::Any;
use std::sync::Arc;
use toni::async_trait;
use toni::http_helpers::HttpRequest;
use toni::traits_helpers::{Provider, ProviderTrait};
use toni::FxHashMap;

/// Service that provides access to configuration
///
/// This service is automatically registered when you import `ConfigModule<T>` in your module.
/// Inject it into your providers to access configuration:
///
/// ```rust,ignore
/// #[provider_struct(
///     pub struct DatabaseService {
///         config: ConfigService<AppConfig>
///     }
/// )]
/// impl DatabaseService {
///     pub fn connect(&self) -> String {
///         let cfg = self.config.get();
///         format!("Connecting to {}", cfg.database_url)
///     }
/// }
/// ```
#[derive(Clone)]
pub struct ConfigService<T: Config> {
    config: Arc<T>,
}

impl<T: Config + Clone + 'static> ConfigService<T> {
    /// Create a new ConfigService instance with the given config
    ///
    /// This is typically handled by the DI system.
    pub fn new(config: Arc<T>) -> Self {
        Self { config }
    }

    /// Get the loaded configuration (clones the config)
    ///
    /// # Usage Notes
    ///
    /// - When using `.get()`, always use a type annotation: `let cfg: AppConfig = self.config.get()`
    /// - Do NOT directly return `.get()` result - use a let binding first:
    ///   ```rust,ignore
    ///   // ❌ BAD - will cause lifetime errors:
    ///   pub fn get_config(&self) -> AppConfig {
    ///       self.config.get()
    ///   }
    ///
    ///   // ✅ GOOD - use type annotation and intermediate binding:
    ///   pub fn get_config(&self) -> AppConfig {
    ///       let cfg: AppConfig = self.config.get();
    ///       cfg
    ///   }
    ///   ```
    /// - Prefer using `.get_ref()` for zero-copy access when possible.
    pub fn get(&self) -> T {
        (*self.config).clone()
    }

    /// Get a reference to the configuration (zero-copy)
    pub fn get_ref(&self) -> &T {
        &self.config
    }
}

// ============================================================================
// ProviderTrait Implementation - Enables ConfigService as Injectable Dependency
// ============================================================================

/// Implement ProviderTrait so ConfigService can be injected as a dependency
#[async_trait]
impl<T: Config> ProviderTrait for ConfigService<T> {
    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _req: Option<&HttpRequest>,
    ) -> Box<dyn Any + Send> {
        // Return a clone of self for injection
        Box::new(self.clone())
    }

    fn get_token(&self) -> String {
        "ConfigService".to_string()
    }

    fn get_token_manager(&self) -> String {
        format!("ConfigService<{}>", std::any::type_name::<T>())
    }
}

// ============================================================================
// Provider Implementation for DI System
// ============================================================================

/// Manager for ConfigService - handles registration with the DI system
pub struct ConfigServiceManager<T: Config> {
    config: Arc<T>,
}

impl<T: Config> ConfigServiceManager<T> {
    pub fn with_config(config: Arc<T>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl<T: Config + Clone + Send + Sync + 'static> Provider for ConfigServiceManager<T> {
    async fn get_all_providers(
        &self,
        _dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ProviderTrait>>> {
        let mut providers = FxHashMap::default();

        // Register struct provider (instance injection pattern)
        // Token: "ConfigService" (matches get_token() and provider lookup)
        let instance = ConfigService {
            config: self.config.clone(),
        };

        providers.insert(
            "ConfigService".to_string(),
            Arc::new(Box::new(instance) as Box<dyn ProviderTrait>),
        );

        providers
    }

    fn get_name(&self) -> String {
        format!("ConfigService<{}>", std::any::type_name::<T>())
    }

    fn get_token(&self) -> String {
        format!("ConfigService<{}>", std::any::type_name::<T>())
    }

    fn get_dependencies(&self) -> Vec<String> {
        vec![] // No dependencies - config is stored in Arc
    }
}
