//! Validating configuration at startup with the `validator` crate.
//!
//! Fields that carry a `#[validate(...)]` attribute are checked when the config
//! loads — invalid values are rejected before the app does any work (fail-fast).
//! Validation is opt-in per field: drop the `validator` derive and the
//! `#[validate]` attrs and the config never touches the validator crate.
//!
//! Run with:           cargo run --example config_validation
//! Trigger a failure:  APP_PORT=80 cargo run --example config_validation
//!   (80 parses as a u16 but is below the validated minimum of 1024)

use toni_config::{Config, ConfigModule};
use validator::Validate;

#[derive(Config, Validate, Clone)]
struct AppConfig {
    #[env("APP_PORT")]
    #[default(8080u16)]
    #[validate(range(min = 1024, max = 65535))]
    pub port: u16,

    #[env("WORKER_THREADS")]
    #[default(4u16)]
    #[validate(range(min = 1, max = 64))]
    pub worker_threads: u16,
}

fn main() {
    match ConfigModule::<AppConfig>::from_env() {
        Ok(module) => {
            let cfg = module.get();
            println!("✅ config valid");
            println!("   port           = {}", cfg.port);
            println!("   worker_threads = {}", cfg.worker_threads);
        }
        Err(e) => {
            eprintln!("❌ config rejected: {e}");
            std::process::exit(1);
        }
    }
}
