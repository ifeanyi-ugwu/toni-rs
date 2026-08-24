//! Default logging in toni applications.
//!
//! toni emits structured log events via the [`tracing`] crate and installs a
//! default subscriber during application creation: pretty-printed output to
//! stderr, filtered by `RUST_LOG` with an `info` fallback. An application
//! that never mentions logging still sees bootstrap events, guard
//! rejections, and panic recoveries — including initialization failures that
//! would otherwise exit silently.
//!
//! ```text
//! cargo run --example logging                  # info and above
//! RUST_LOG=toni=debug cargo run --example logging
//! RUST_LOG=off cargo run --example logging     # silence at runtime
//! ```
//!
//! To bring your own backend (JSON output, `tracing-appender`,
//! OpenTelemetry), install a global subscriber before creating the
//! application — the default backs off whenever one is already set:
//!
//! ```text
//! tracing_subscriber::fmt().json().init();
//! let app = ToniFactory::create(AppModule).await?;
//! ```
//!
//! To compile the default logger out entirely, disable the crate's default
//! features: `toni = { version = "0.2", default-features = false }`.

use toni::*;
use toni_axum::AxumAdapter;

#[controller("/hello")]
struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    fn hello(&self) -> Body {
        Body::text("Hello, toni!".to_string())
    }
}

#[module(controllers: [HelloController], providers: [])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo run --example logging");
    println!("RUST_LOG=debug cargo run --example logging");
    println!();
    println!("  GET http://127.0.0.1:3000/hello");
    println!();

    let mut app = ToniFactory::create(AppModule).await?;

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();

    app.start().await?;
    Ok(())
}
