//! Simple test to verify instance injection for providers only (no controllers)

use toni_config::{Config, ConfigModule, ConfigService};
use toni::{module, provider_struct};

#[derive(Config, Clone)]
struct SimpleConfig {
    #[default("test".to_string())]
    pub value: String,
}

#[provider_struct(
    pub struct SimpleService {
        config: ConfigService<SimpleConfig>
    }
)]
impl SimpleService {
    pub fn get_value(&self) -> String {
        // Direct field access with new instance injection!
        self.config.get_ref().value.clone()
    }
}

#[module(
    imports: [ConfigModule::<SimpleConfig>::new()],
    providers: [SimpleService],
)]
impl TestModule {}

#[test]
fn test_simple_provider_instance_injection() {
    // This test just needs to compile successfully
    // It verifies that the macro generates valid code
    println!("Provider instance injection compiles!");
}
