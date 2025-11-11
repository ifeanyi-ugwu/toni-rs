//! Test for constructor-based dependency injection
//!
//! This test verifies:
//! 1. Auto-detected new() constructor with DI
//! 2. Explicit init attribute with custom constructor
//! 3. Default fallback for structs without constructors

use toni::{injectable, module};

// ============= Base Service =============

#[injectable(pub struct ConfigService {
    value: String,
})]
impl ConfigService {
    pub fn new() -> Self {
        Self {
            value: "test_config".to_string(),
        }
    }

    pub fn get_value(&self) -> String {
        self.value.clone()
    }
}

// ============= Example 1: Auto-detected new() =============

#[injectable(pub struct AutoDetectedService {
    config_value: String,
})]
impl AutoDetectedService {
    // new() is auto-detected - config param will be DI-resolved
    pub fn new(config: ConfigService) -> Self {
        Self {
            config_value: config.get_value(),
        }
    }

    pub fn get_value(&self) -> String {
        format!("AutoDetected: {}", self.config_value)
    }
}

// ============= Example 2: Explicit init with custom name =============

#[injectable(init = "create", pub struct ExplicitInitService {
    combined: String,
})]
impl ExplicitInitService {
    // Custom constructor name - both params will be DI-resolved
    pub fn create(config: ConfigService, auto: AutoDetectedService) -> Self {
        Self {
            combined: format!("{} + {}", config.get_value(), auto.get_value()),
        }
    }

    pub fn get_combined(&self) -> String {
        self.combined.clone()
    }
}

// ============= Example 3: Default fallback (no constructor) =============

#[injectable]
pub struct DefaultFallbackService {
    name: String, // Uses String::default() = ""
    count: i32,   // Uses i32::default() = 0
}

impl DefaultFallbackService {
    pub fn get_info(&self) -> String {
        format!("name='{}', count={}", self.name, self.count)
    }
}

// ============= Example 4: Mixed - new() with multiple params =============

#[injectable(pub struct MultiParamService {
    data: String,
})]
impl MultiParamService {
    pub fn new(
        config: ConfigService,
        auto: AutoDetectedService,
        explicit: ExplicitInitService,
    ) -> Self {
        Self {
            data: format!(
                "config={}, auto={}, explicit={}",
                config.get_value(),
                auto.get_value(),
                explicit.get_combined()
            ),
        }
    }

    pub fn get_data(&self) -> String {
        self.data.clone()
    }
}

// ============= Test Module =============

#[module(
    providers: [
        ConfigService,
        AutoDetectedService,
        ExplicitInitService,
        DefaultFallbackService,
        MultiParamService,
    ],
    exports: [
        ConfigService,
        AutoDetectedService,
        ExplicitInitService,
        DefaultFallbackService,
        MultiParamService,
    ],
)]
impl ConstructorTestModule {}

// ============= Tests =============

#[cfg(test)]
mod tests {
    use super::*;
}
