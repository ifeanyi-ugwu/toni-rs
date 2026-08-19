//! Test fixture for the `rpc_controller_injection` integration test.
//!
//! Injects an RPC controller into an ordinary provider. A controller is declared in `controllers:`
//! and is reached by pattern, so its token is not in the provider store: the injector finds nothing
//! under it and `create_application_context` logs the failure and exits with status 1.
//!
//! A subprocess is required because the only public trigger path calls `std::process::exit`.

#![allow(dead_code)]

use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::*;
use toni_macros::{message_pattern, new, patterns, rpc_controller};
use tracing_subscriber::{fmt, EnvFilter};

#[rpc_controller]
pub struct OrdersController {}

#[patterns]
impl OrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.get")]
    async fn get(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[injectable]
pub struct OrdersReporter {
    #[inject]
    controller: OrdersController,
}

#[module(controllers: [OrdersController], providers: [OrdersReporter])]
impl AppModule {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    fmt()
        .with_env_filter(EnvFilter::new("error"))
        .with_target(false)
        .init();

    let _ctx = ToniFactory::create_application_context(AppModule).await;
}
