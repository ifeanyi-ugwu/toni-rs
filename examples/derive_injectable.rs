//! `#[derive(Injectable)]` — field-injection providers without the struct-in-macro form.
//!
//! Every provider here is a plain struct with a normal `impl`. Dependencies are declared
//! as `#[inject]` fields; owned state uses `#[default(...)]`; scope is set with a companion
//! `#[provider(scope = "...")]` attribute. Compare with `provider_patterns.rs`, which threads
//! the struct definition through the `#[injectable(pub struct ... { ... })]` attribute.
//!
//! Run with:  cargo run --example derive_injectable

use toni::{Injectable, module, toni_factory::ToniFactory};

#[derive(Clone, Injectable)]
pub struct Config {
    #[default("production".to_string())]
    env: String,
}

impl Config {
    pub fn env(&self) -> &str {
        &self.env
    }
}

// `#[provider(scope = "...")]` is the companion attribute that replaces the
// `#[injectable(scope = "...")]` argument. A fresh Logger is built per resolution.
#[derive(Clone, Injectable)]
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

// Field injection: `config` and `logger` are resolved from the container and moved into
// the fields at build time. No `new()`, no struct threaded through a macro attribute.
#[derive(Clone, Injectable)]
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

// `init = "new"` redirects construction through `Self::new(deps…)`: the resolved `#[inject]`
// fields are passed in declaration order. `new` owns assembly, so it can derive extra state
// (here `prefix`) without a `#[default]` field. A missing/mis-typed `new` fails loudly.
#[derive(Clone, Injectable)]
#[provider(init = "new")]
pub struct Banner {
    #[inject]
    config: Config,
    prefix: String,
}

impl Banner {
    pub fn new(config: Config) -> Self {
        let prefix = format!("<{}>", config.env());
        Self { config, prefix }
    }

    pub fn render(&self) -> String {
        format!("{} banner", self.prefix)
    }
}

#[module(providers: [Config, Logger, Greeter, Banner])]
struct AppModule {}

#[tokio::main]
async fn main() {
    println!("🔧 #[derive(Injectable)] field injection\n");

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
        .expect("Banner resolves via Self::new(config)");
    println!("  {}", banner.render());

    println!("\n✅ resolved field-injection + init=\"new\" from plain structs + derives");
}
