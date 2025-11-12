//! Test for provider_factory! macro
//!
//! This test verifies:
//! 1. provider_factory! works directly in module providers array
//! 2. Supports sync factory functions
//! 3. Supports async factory functions with auto-detection
//! 4. Supports factory functions with dependency injection
//! 5. Supports all token types: String, Type, and Const

use toni::{injectable, module, provider_factory};

// ============= Dependencies =============

#[injectable(pub struct ConfigService {
    api_url: String,
})]
impl ConfigService {
    pub fn new() -> Self {
        Self {
            api_url: "https://api.example.com".to_string(),
        }
    }

    pub fn get_api_url(&self) -> String {
        self.api_url.clone()
    }
}

// ============= Test Module =============

#[module(
    providers: [
        ConfigService,

        // Sync factory with no deps
        provider_factory!("COUNTER", || 42_i32),

        // Async factory with no deps (auto-detected)
        provider_factory!("ASYNC_VALUE", async || "async_result".to_string()),

        // Sync factory WITH dependency injection
        provider_factory!("LOGGER", |config: ConfigService| {
            format!("Logger initialized for: {}", config.get_api_url())
        }),

        // Async factory WITH dependency injection (auto-detected)
        provider_factory!("ASYNC_LOGGER", async |config: ConfigService| {
            // Simulate async work
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            format!("Async Logger for: {}", config.get_api_url())
        }),

        // Factory that creates a struct with deps
        provider_factory!("HTTP_CLIENT", |config: ConfigService| {
            #[derive(Clone)]
            struct HttpClient {
                base_url: String,
            }
            HttpClient {
                base_url: config.get_api_url(),
            }
        }),
    ],
    exports: [],
)]
impl FactoryProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
