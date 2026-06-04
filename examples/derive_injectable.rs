//! `#[provider]` — field-injection providers as plain structs.
//!
//! Each provider is a normal struct tagged `#[provider]`, with a normal `impl`. Dependencies are
//! `#[inject]` fields; owned state uses `#[default(...)]`; scope is `#[provider(scope = "...")]`; a
//! `#[new]` constructor injects dependencies that need not be stored as fields. No `Clone` derive,
//! no struct threaded through a macro attribute.
//!
//! Run with:  cargo run --example derive_injectable

use toni::{module, new, provider, toni_factory::ToniFactory};

#[provider]
pub struct Config {
    #[default("production".to_string())]
    env: String,
}

impl Config {
    pub fn env(&self) -> &str {
        &self.env
    }
}

// A fresh Logger is built per resolution.
#[provider(scope = "transient")]
pub struct Logger {
    #[default("info".to_string())]
    level: String,
}

impl Logger {
    pub fn line(&self, msg: &str) -> String {
        format!("[{}] {}", self.level, msg)
    }
}

// Field injection: `config` and `logger` are resolved from the container and moved into the fields
// at build time.
#[provider]
pub struct Greeter {
    #[inject]
    config: Config,
    #[inject]
    logger: Logger,
}

impl Greeter {
    pub fn greet(&self) -> String {
        self.logger
            .line(&format!("hello from {} mode", self.config.env()))
    }
}

// `#[new]` marks a DI constructor: each parameter is resolved from the container and passed in.
// Here `config` is injected and used, but NOT stored — only the derived `prefix` is a field, which
// plain field injection can't express.
#[provider]
pub struct Banner {
    prefix: String,
}

impl Banner {
    #[new]
    fn new(config: Config) -> Self {
        Self {
            prefix: format!("<{}>", config.env()),
        }
    }

    pub fn render(&self) -> String {
        format!("{} banner", self.prefix)
    }
}

#[module(providers: [Config, Logger, Greeter, Banner])]
struct AppModule {}

#[tokio::main]
async fn main() {
    println!("🔧 #[provider] field injection\n");

    let app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    let greeter = app
        .get::<Greeter>()
        .await
        .expect("Greeter resolves from DI with Config + Logger injected");

    println!("  {}", greeter.greet());

    let banner = app
        .get::<Banner>()
        .await
        .expect("Banner resolves via its #[new] constructor");
    println!("  {}", banner.render());

    println!("\n✅ resolved field injection + a #[new] constructor (non-stored dep) from plain structs");
}
