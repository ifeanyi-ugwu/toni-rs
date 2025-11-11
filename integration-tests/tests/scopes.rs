use toni::injectable;
use toni_config::{Config, ConfigService};

#[derive(Clone, Debug, Config)]
struct TestConfig {
    value: String,
}

impl TestConfig {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            value: "test".to_string(),
        })
    }
}

// Test Singleton Scope (Default)
#[injectable(
    pub struct SingletonService {
        #[inject]
        config: ConfigService<TestConfig>
    }
)]
impl SingletonService {
    pub fn get_value(&self) -> String {
        self.config.get_ref().value.clone()
    }
}

// Test Request Scope
#[injectable(
    scope = "request",
    pub struct RequestService {
        #[inject]
        config: ConfigService<TestConfig>
    }
)]
impl RequestService {
    pub fn get_value(&self) -> String {
        format!("request:{}", self.config.get_ref().value)
    }
}

// Test Transient Scope
#[injectable(
    scope = "transient",
    pub struct TransientService {
        #[inject]
        config: ConfigService<TestConfig>
    }
)]
impl TransientService {
    pub fn get_value(&self) -> String {
        format!("transient:{}", self.config.get_ref().value)
    }
}

#[test]
fn test_singleton_scope_compiles() {
    println!("Singleton scope provider compiles successfully!");
}

#[test]
fn test_request_scope_compiles() {
    println!("Request scope provider compiles successfully!");
}

#[test]
fn test_transient_scope_compiles() {
    println!("Transient scope provider compiles successfully!");
}

#[test]
fn test_all_scopes_compile() {
    println!("All three scopes (singleton, request, transient) compile successfully!");
    println!("Singleton: Default, created once at startup");
    println!("Request: Created once per HTTP request");
    println!("Transient: Created every time it's injected");
}
