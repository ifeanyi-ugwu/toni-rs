use toni_config::{Config, ConfigService};
use toni_macros::injectable;

#[derive(Clone, Config)]
pub struct AppConfig {
    pub app_name: String,
    pub port: u16,
}

#[injectable(
    pub struct AppService {
        #[inject]
        config: ConfigService<AppConfig>
    }
)]
impl AppService {
    pub fn get_app_name(&self) -> String {
        // Direct field access! No type annotation needed!
        self.config.get_ref().app_name.clone()
    }

    pub fn get_port(&self) -> u16 {
        self.config.get_ref().port
    }
}

fn main() {
    println!("Instance injection test compiled successfully!");
}
