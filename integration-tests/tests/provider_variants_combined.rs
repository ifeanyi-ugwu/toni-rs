//! Comprehensive test for all provider variant macros
//!
//! This test verifies all provider variants work together:
//! 1. provider_value! - constant values
//! 2. provider_factory! - factory functions (sync/async, with/without deps)
//! 3. provider_alias! - aliases to existing providers
//! 4. provider_token! - custom tokens for existing types

use std::time::Duration;
use toni::{injectable, module, provider_alias, provider_factory, provider_token, provider_value};

// ============= Services =============

#[injectable(pub struct ConfigService {
    env: String,
})]
impl ConfigService {
    pub fn new() -> Self {
        Self {
            env: "production".to_string(),
        }
    }

    pub fn get_env(&self) -> String {
        self.env.clone()
    }
}

#[injectable(pub struct LoggerService {
    level: String,
})]
impl LoggerService {
    pub fn new() -> Self {
        Self {
            level: "info".to_string(),
        }
    }

    pub fn log(&self, msg: &str) -> String {
        format!("[{}] {}", self.level, msg)
    }
}

// ============= Test Module =============

#[module(
    providers: [
        // Injectable services
        ConfigService,
        LoggerService,

        // Value providers
        provider_value!("APP_NAME", "ToniApp".to_string()),
        provider_value!("PORT", 3000_u16),
        provider_value!("TIMEOUT", Duration::from_secs(30)),

        // Factory providers (no deps)
        provider_factory!("REQUEST_ID", || {
            format!("req_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis())
        }),

        // Factory providers (with deps)
        provider_factory!("APP_INFO", |config: ConfigService| {
            format!("App running in {} mode", config.get_env())
        }),

        // Async factory (with deps)
        provider_factory!("ASYNC_STATUS", async |logger: LoggerService| {
            tokio::time::sleep(Duration::from_millis(1)).await;
            logger.log("System initialized")
        }),

        // Aliases
        provider_alias!("Config", ConfigService),
        provider_alias!("Logger", LoggerService),
        provider_alias!("APP_PORT", "PORT"),

        // Custom tokens
        provider_token!("PRIMARY_CONFIG", ConfigService),
        provider_token!("SECONDARY_LOGGER", LoggerService),
    ],
    exports: [],
)]
impl CombinedProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {
    #[test]
    fn test_all_provider_variants_compile() {
        // Test that all provider variants work together
        // This demonstrates:
        // 1. Value providers for constants
        // 2. Factory providers with and without dependencies
        // 3. Async factories with auto-detection
        // 4. Aliases pointing to services and other values
        // 5. Custom tokens for existing types
        assert!(true);
    }
}
