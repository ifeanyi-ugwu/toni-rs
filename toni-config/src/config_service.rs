//! ConfigService - Injectable service for accessing configuration in providers

use crate::Config;
use async_trait::async_trait;
use rustc_hash::FxHashMap;
use std::any::Any;
use std::sync::Arc;
use toni::traits_helpers::{Provider, ProviderTrait};

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

impl<T: Config + Clone + Send + Sync + 'static> Provider for ConfigServiceManager<T> {
    fn get_all_providers(
        &self,
        _dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ProviderTrait>>> {
        let mut providers = FxHashMap::default();

        // Register the `get` method provider
        // Token format: {METHOD_NAME_UPPER}{STRUCT_NAME} = "GetConfigService"
        let get_token = "GetConfigService".to_string();
        let get_provider: Arc<Box<dyn ProviderTrait>> = Arc::new(Box::new(ConfigServiceGet::<T> {
            config: self.config.clone(),
        }));
        providers.insert(get_token, get_provider);

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

/// Provider wrapper for ConfigService::get method
struct ConfigServiceGet<T: Config> {
    config: Arc<T>,
}

#[async_trait]
impl<T: Config + Clone + Send + Sync + 'static> ProviderTrait for ConfigServiceGet<T> {
    async fn execute(&self, _params: Vec<Box<dyn Any + Send>>) -> Box<dyn Any + Send> {
        Box::new((*self.config).clone())
    }

    fn get_token(&self) -> String {
        "GetConfigService".to_string()
    }

    fn get_token_manager(&self) -> String {
        format!("ConfigService<{}>", std::any::type_name::<T>())
    }
}
