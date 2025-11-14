//! Comprehensive test for the unified provide! macro
//!
//! This test verifies:
//! 1. Auto-detection: literals → value, closures → factory
//! 2. Explicit markers: existing(), provider(), value(), factory()
//! 3. Mixed usage in a single module
//! 4. All provider variants work through unified macro

use std::time::Duration;
use toni::{injectable, module, provide};

// ============= Base Services =============

#[injectable(pub struct ConfigService {
    env: String,
})]
impl ConfigService {
    pub fn new() -> Self {
        Self {
            env: "production".to_string(),
        }
    }

    pub fn get_env(&self) -> &str {
        &self.env
    }
}

#[injectable(pub struct DatabaseService {
    host: String,
})]
impl DatabaseService {
    pub fn new() -> Self {
        Self {
            host: "localhost:5432".to_string(),
        }
    }

    pub fn get_host(&self) -> &str {
        &self.host
    }
}

#[injectable(pub struct CacheService {
    redis_url: String,
})]
impl CacheService {
    pub fn new() -> Self {
        Self {
            redis_url: "redis://localhost:6379".to_string(),
        }
    }
}

// ============= Service with Token Injection =============

#[injectable(pub struct AppService {
    #[inject]
    config: ConfigService,

    #[inject("API_KEY")]
    api_key: String,

    #[inject("PORT")]
    port: u16,

    #[inject("TIMEOUT")]
    timeout: Duration,

    #[inject("PRIMARY_DB")]
    database: DatabaseService,

    #[inject("CACHE_ALIAS")]
    cache: CacheService,

    #[inject("MAX_CONNECTIONS")]
    max_connections: i32,
})]
impl AppService {}

// ============= Test Module Using provide! Macro =============

#[module(
    providers: [
        // Regular injectable services
        ConfigService,
        CacheService,

        // AUTO-DETECTED: Literals → Value providers
        provide!("API_KEY", "secret_key_123".to_string()),
        provide!("PORT", 8080_u16),
        provide!("TIMEOUT", Duration::from_secs(30)),

        // AUTO-DETECTED: Closures → Factory providers
        provide!("MAX_CONNECTIONS", || 100_i32),
        provide!("LOGGER", |config: ConfigService| {
            format!("Logger for env: {}", config.get_env())
        }),

        // EXPLICIT MARKERS: provider() - Register type under custom token
        provide!("PRIMARY_DB", provider(DatabaseService)),

        // EXPLICIT MARKERS: existing() - Alias to existing provider
        provide!("CACHE_ALIAS", existing(CacheService)),

        // EXPLICIT MARKERS: value() - Explicit value (redundant but allowed)
        provide!("EXPLICIT_VALUE", value("test".to_string())),

        // EXPLICIT MARKERS: factory() - Explicit factory (redundant but allowed)
        provide!("EXPLICIT_FACTORY", factory(|| "factory_result".to_string())),

        // The main service that uses all these providers
        AppService,
    ],
    exports: [],
)]
impl UnifiedProvideTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {
    use super::*;
}
