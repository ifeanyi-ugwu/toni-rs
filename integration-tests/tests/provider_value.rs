//! Test for provider_value! macro
//!
//! This test verifies:
//! 1. provider_value! works directly in module providers array
//! 2. Supports string literal tokens: provider_value!("TOKEN", value)
//! 3. Supports all token types: String, Type, and Const

use std::time::Duration;
use toni::{module, provider_value};

// a const token for testing
use toni::di::APP_GUARD;

// ============= Test Module =============

#[module(
    providers: [
        // String literal tokens
        provider_value!("API_KEY", "secret_key_123".to_string()),
        provider_value!("TIMEOUT", Duration::from_secs(30)),
        provider_value!("PORT", 8080_u16),

        // Const token (SCREAMING_SNAKE_CASE)
        provider_value!(APP_GUARD, "global_guard".to_string()),
        // Type token (uses type name as token)
        provider_value!(Duration, Duration::from_secs(60)),
    ],
    exports: [],
)]
impl ValueProviderTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {}
