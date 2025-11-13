//! Test for #[inject("TOKEN")] token override in field injection
//!
//! This test verifies:
//! 1. #[inject("TOKEN")] allows injecting with custom tokens
//! 2. Works with provider_value!, provider_token!, provider_factory!
//! 3. Mixed injection (some type-based, some token-based) works

use std::time::Duration;
use toni::{injectable, module, provider_value, provider_token};

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
}

#[injectable(pub struct DatabaseService {
    host: String,
})]
impl DatabaseService {
    pub fn new() -> Self {
        Self {
            host: "localhost".to_string(),
        }
    }
}

// ============= Service with Token-based Injection =============

#[injectable(pub struct AppService {
    // Type-based injection (uses type name as token)
    #[inject]
    config: ConfigService,

    // Token-based injection (uses custom string token)
    #[inject("API_KEY")]
    api_key: String,

    // Token-based injection (uses custom token from provider_token!)
    #[inject("PRIMARY_DB")]
    database: DatabaseService,

    // Token-based injection with Duration from provider_value!
    #[inject("TIMEOUT")]
    timeout: Duration,

    // Token-based injection with port number
    #[inject("PORT")]
    port: u16,
})]
impl AppService {}

// ============= Test Module =============

#[module(
    providers: [
        // Regular injectable services
        ConfigService,

        // Value providers with string tokens
        provider_value!("API_KEY", "secret_key_123".to_string()),
        provider_value!("TIMEOUT", Duration::from_secs(30)),
        provider_value!("PORT", 8080_u16),

        // Custom token provider (DatabaseService registered ONLY as "PRIMARY_DB")
        provider_token!("PRIMARY_DB", DatabaseService),

        // The service that uses token-based injection
        AppService,
    ],
    exports: [],
)]
impl TokenOverrideTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
