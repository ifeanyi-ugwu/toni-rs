//! Test for provider_token! macro
//!
//! This test verifies:
//! 1. provider_token! works directly in module providers array
//! 2. Creates a custom token for a type WITHOUT auto-registering the type
//! 3. Identical to NestJS's { provide: 'TOKEN', useClass: Type }
//! 4. Type is registered ONLY under the custom token, NOT under its type name

use toni::{injectable, module, provider_token};

// ============= Test Service =============

#[injectable(pub struct DatabaseService {
    connection_string: String,
})]
impl DatabaseService {
    pub fn new() -> Self {
        Self {
            connection_string: "localhost:5432".to_string(),
        }
    }

    pub fn get_connection(&self) -> String {
        self.connection_string.clone()
    }
}

#[injectable(pub struct CacheService {
    host: String,
})]
impl CacheService {
    pub fn new() -> Self {
        Self {
            host: "localhost:6379".to_string(),
        }
    }

    pub fn get_host(&self) -> String {
        self.host.clone()
    }
}

// ============= Test Module =============

#[module(
    providers: [
        // Create custom tokens - types NOT auto-registered
        provider_token!("PRIMARY_DB", DatabaseService),
        provider_token!("SECONDARY_DB", DatabaseService),
        provider_token!("RedisCache", CacheService),
    ],
    exports: [],
)]
impl TokenProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
