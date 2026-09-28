//! Provider declarations: a value, a factory, a second slot, an alias, a trait binding
//!
//! Shows the forms of `provide!`, a consumer that injects from each, and how to
//! retrieve a provider from the DI container directly — no HTTP server
//! required.
//!
//! Run with:  cargo run --example provider_patterns

use std::sync::Arc;
use std::time::Duration;
use ulo::{UloFactory, injectable, key, module, new, provide};
// ---- providers ---------------------------------------------------------------

#[injectable]
pub struct ConfigService {
    env: String,
}
impl ConfigService {
    #[new]
    pub fn new() -> Self {
        Self {
            env: "production".to_string(),
        }
    }

    pub fn get_env(&self) -> String {
        self.env.clone()
    }
}

#[injectable]
pub struct LoggerService {
    level: String,
}
impl LoggerService {
    #[new]
    pub fn new() -> Self {
        Self {
            level: "info".to_string(),
        }
    }

    pub fn log(&self, msg: &str) -> String {
        format!("[{}] {}", self.level, msg)
    }
}

pub trait Greeter: Send + Sync {
    fn greet(&self) -> String;
}

pub struct English;

impl Greeter for English {
    fn greet(&self) -> String {
        "hello".to_string()
    }
}

// A type is its own key. A marker names a second slot, and says what it holds.

key!(pub AppName: String);
key!(pub Port: u16);
key!(pub AppPort: u16);
key!(pub AppStatus: String);
key!(pub PrimaryConfig: ConfigService);

// ---- consumer ----------------------------------------------------------------
//
// Injects from each declaration so the resolved output is observable.

#[injectable]
pub struct AppInfo {
    // a value under a marker
    #[inject(AppName)]
    name: String,

    #[inject(Port)]
    port: u16,

    // an alias: the slot `AppPort` answers with the binding under `Port`
    #[inject(AppPort)]
    app_port: u16,

    // a factory with no key, bound under the type it builds
    #[inject]
    timeout: Duration,

    // an async factory under a marker, built from a dependency
    #[inject(AppStatus)]
    status: String,

    // a type's own declaration under a second slot
    #[inject(PrimaryConfig)]
    primary: ConfigService,

    // a trait bound to an implementation
    #[inject]
    greeter: Arc<dyn Greeter>,
}
impl AppInfo {
    fn print(&self) {
        println!("  app_name  (value under a marker):   {}", self.name);
        println!("  port      (value under a marker):   {}", self.port);
        println!("  app_port  (alias):                  {}", self.app_port);
        println!("  timeout   (keyless factory):        {:?}", self.timeout);
        println!("  status    (async factory + dep):    {}", self.status);
        println!(
            "  primary   (type under a marker):    env={}",
            self.primary.get_env()
        );
        println!(
            "  greeter   (trait binding):          {}",
            self.greeter.greet()
        );
    }
}

// ---- module ------------------------------------------------------------------

#[module(
    providers: [
        ConfigService,
        LoggerService,

        provide!(AppName => "UloApp".to_string()),
        provide!(Port => 3000_u16),
        provide!(AppPort => alias Port),

        // a factory is async; this one reads no dependency
        provide!(async || Duration::from_secs(60)),
        provide!(AppStatus => async |logger: LoggerService| {
            tokio::time::sleep(Duration::from_millis(1)).await;
            logger.log("System initialized")
        }),

        provide!(PrimaryConfig => ConfigService),
        provide!(dyn Greeter => value English),

        AppInfo,
    ],
    exports: [],
)]
impl ProviderPatternsModule {}

// ---- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    println!("🔧 ulo provider patterns\n");

    let app = UloFactory::new()
        .create_with(ProviderPatternsModule)
        .await
        .unwrap();

    let info = app
        .get::<AppInfo>()
        .await
        .expect("AppInfo should resolve — check each marker is declared");

    println!("Resolved values:");
    info.print();
    println!(
        "\nRead directly: port = {}",
        app.get_key::<Port>().await.unwrap()
    );
}
