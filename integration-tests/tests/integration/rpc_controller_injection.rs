#![allow(dead_code)]

use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::*;
use toni_macros::{message_pattern, new, patterns, rpc_controller};

use crate::common::panic_message;

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

/// An RPC controller is a dispatch target: it is reached by pattern and nothing may hold it.
/// Declared in `controllers:`, its token is not in the provider store, so injecting it into an
/// ordinary provider fails resolution at init.
///
/// The other half of the refusal is not reachable from here: listing a dispatch target in
/// `providers:` does not compile, because the macro emits no provider factory for one.
#[test]
fn an_rpc_controller_is_not_resolvable_as_a_dependency() {
    let message = panic_message(|| ToniFactory::create_application_context(AppModule));

    assert!(
        message.contains("Dependency not found"),
        "expected an unresolved-dependency failure, got:\n{message}"
    );
    assert!(
        message.contains("OrdersController"),
        "the failure should name the controller, got:\n{message}"
    );
}
