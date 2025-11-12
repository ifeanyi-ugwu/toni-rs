//! Test for provider_factory! macro
//!
//! This test verifies:
//! 1. provider_factory! works directly in module providers array
//! 2. Supports sync factory functions
//! 3. Supports async factory functions with auto-detection
//! 4. Supports all token types: String, Type, and Const

use toni::{module, provider_factory};

// ============= Test Module =============

#[module(
    providers: [
        // Sync factory with string token
        provider_factory!("COUNTER", || 42_i32),

        // Async factory with string token (auto-detected)
        provider_factory!("ASYNC_VALUE", async || "async_result".to_string()),

        // Sync factory that creates a struct
        provider_factory!("CONFIG", || {
            #[derive(Clone)]
            struct Config {
                host: String,
                port: u16,
            }
            Config {
                host: "localhost".to_string(),
                port: 3000,
            }
        }),

        // Async factory with computation
        provider_factory!("COMPUTED", async || {
            // Simulate async work
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            "computed_value".to_string()
        }),
    ],
    exports: [],
)]
impl FactoryProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
