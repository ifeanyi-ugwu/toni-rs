//! Test fixture for the `circular_dependency` integration test.
//!
//! Builds a cross-module provider cycle that reaches the injector's Phase-1 stall.
//! Global modules keep the import graph acyclic — both get ordered and reach Phase 1 —
//! while the providers still form a `ServiceA` <-> `ServiceB` cycle. `#[new]` constructor
//! injection consumes each dependency without storing it, so neither struct embeds the
//! other (finite size, compiles). `create_application_context` logs the diagnostic and
//! exits with status 1; the test asserts the logged message.
//!
//! A subprocess is required because the only public trigger path calls `std::process::exit`.

#![allow(dead_code)]

use toni::*;
use tracing_subscriber::{fmt, EnvFilter};

#[injectable]
pub struct ServiceA {
    name: String,
}

impl ServiceA {
    #[new]
    pub fn new(_b: ServiceB) -> Self {
        Self { name: "a".into() }
    }
}

#[injectable]
pub struct ServiceB {
    name: String,
}

impl ServiceB {
    #[new]
    pub fn new(_a: ServiceA) -> Self {
        Self { name: "b".into() }
    }
}

#[module(providers: [ServiceA], exports: [ServiceA], global: true)]
impl ModuleA {}

#[module(providers: [ServiceB], exports: [ServiceB], global: true)]
impl ModuleB {}

#[module(imports: [ModuleA, ModuleB])]
impl AppModule {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    fmt()
        .with_env_filter(EnvFilter::new("error"))
        .with_target(false)
        .init();

    let _ctx = ToniFactory::create_application_context(AppModule).await;
}
