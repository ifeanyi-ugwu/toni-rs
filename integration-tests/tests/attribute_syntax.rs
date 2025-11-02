//! Test for Attribute Syntax - struct annotated, not contained
//!
//! New syntax: #[injectable] pub struct Foo { ... }
//! Old syntax: #[injectable(pub struct Foo { ... })]

use toni::{injectable, module};
use toni_config::{Config, ConfigModule, ConfigService};

#[derive(Config, Clone)]
struct TestConfig {
    #[default("test".to_string())]
    pub value: String,
}

// Test 1: New syntax - basic
#[injectable]
pub struct SimpleService {
    #[inject]
    config: ConfigService<TestConfig>,
}

impl SimpleService {
    pub fn get_value(&self) -> String {
        self.config.get().value
    }
}

// Test 2: New syntax with scope
#[injectable(scope = "request")]
pub struct RequestService {
    #[inject]
    config: ConfigService<TestConfig>,
}

impl RequestService {
    pub fn get_value(&self) -> String {
        self.config.get().value
    }
}

// Test 3: New syntax with owned fields
#[injectable]
pub struct MixedService {
    #[inject]
    config: ConfigService<TestConfig>,

    #[default(100)]
    max_size: usize,
}

impl MixedService {
    pub fn get_max_size(&self) -> usize {
        self.max_size
    }
}

// Test 4: New syntax with custom init
#[injectable(init = "create")]
pub struct CustomInitService {
    #[inject]
    config: ConfigService<TestConfig>,

    prefix: String,
    count: usize,
}

impl CustomInitService {
    fn create(config: ConfigService<TestConfig>) -> Self {
        Self {
            config,
            prefix: "custom:".to_string(),
            count: 42,
        }
    }

    pub fn get_prefix(&self) -> String {
        self.prefix.clone()
    }
}

// Module
#[module(
    imports: [ConfigModule::<TestConfig>::new()],
    providers: [SimpleService, RequestService, MixedService, CustomInitService],
)]
impl TestModule {}

#[test]
fn test_attribute_syntax_compiles() {
    println!("Pattern C syntax test compiles successfully!");
}
