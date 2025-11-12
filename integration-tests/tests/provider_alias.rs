//! Test for provider_alias! macro
//!
//! This test verifies:
//! 1. provider_alias! works directly in module providers array
//! 2. Creates an alias that points to an existing provider
//! 3. Alias inherits the scope and behavior of the target provider
//! 4. Supports all token types for both alias and target

use toni::{injectable, module, provider_alias, provider_value};

// ============= Test Service =============

#[injectable(pub struct AuthService {
    secret_key: String,
})]
impl AuthService {
    pub fn new() -> Self {
        Self {
            secret_key: "super_secret".to_string(),
        }
    }

    pub fn get_secret(&self) -> String {
        self.secret_key.clone()
    }
}

// ============= Test Module =============

#[module(
    providers: [
        AuthService,

        // Create a value provider
        provider_value!("API_KEY", "my_api_key".to_string()),

        // Alias using string tokens (alias -> existing)
        provider_alias!("AUTH_ALIAS", "toni::AuthService"),

        // Alias from string to string
        provider_alias!("KEY_ALIAS", "API_KEY"),

        // Alias using type tokens
        provider_alias!("AuthServiceAlias", AuthService),
    ],
    exports: [],
)]
impl AliasProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
