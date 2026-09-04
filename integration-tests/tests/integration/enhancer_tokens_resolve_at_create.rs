#![allow(dead_code)]

use std::pin::Pin;

use crate::common::NotServed;
use futures_util::Stream;
use toni::context::{GrpcContext, RpcContext};
use toni::extractors::{Inbound, Payload};
use toni::rpc::{RpcData, RpcError};
use toni::traits_helpers::Guard;
use toni::*;
use toni_macros::{controller, grpc_methods, message_pattern, new, patterns, use_guards};

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

// `OrdersServer` reads as unused here — `#[grpc_methods]` names it in the code it emits.
use orders_pb::orders_server::{Orders, OrdersServer};

// Real guard types that are absent from every `providers:` list, so their tokens resolve
// against nothing in the role registry.

#[injectable]
pub struct UnregisteredRpcGuard {}
impl UnregisteredRpcGuard {}

#[async_trait]
impl Guard<RpcContext> for UnregisteredRpcGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        true
    }
}

#[injectable]
pub struct UnregisteredGrpcGuard {}
impl UnregisteredGrpcGuard {}

#[async_trait]
impl Guard<GrpcContext> for UnregisteredGrpcGuard {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        true
    }
}

// ---- RPC ---------------------------------------------------------------------

#[controller]
pub struct GuardedOrdersController {}

#[patterns]
#[use_guards(UnregisteredRpcGuard)]
impl GuardedOrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.get")]
    async fn get(&self, data: RpcData) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [GuardedOrdersController])]
impl RpcAppModule {}

/// Enhancer tokens resolve while instances load, so a token naming a guard no module provides
/// fails `create()` — before any adapter or socket exists.
#[tokio::test]
async fn a_misdeclared_rpc_enhancer_token_fails_create() {
    let message = ToniFactory::create_application_context(RpcAppModule)
        .await
        .err()
        .expect("a misdeclared enhancer token must fail create")
        .to_string();

    assert!(
        message.contains("not found in registry"),
        "expected an unresolved-enhancer failure, got:\n{message}"
    );
    assert!(
        message.contains("UnregisteredRpcGuard"),
        "the failure should name the guard, got:\n{message}"
    );
}

// ---- gRPC --------------------------------------------------------------------

#[controller]
pub struct GuardedGrpcService {}

impl GuardedGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(orders_pb::orders_server::Orders)]
#[use_guards(UnregisteredGrpcGuard)]
impl GuardedGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::CreateOrderResponse, NotServed> {
        Ok(orders_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<orders_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<orders_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::BulkCreateResponse, NotServed> {
        Ok(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        })
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<orders_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<orders_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [GuardedGrpcService])]
impl GrpcAppModule {}

/// The same phase pin on the gRPC path, which resolves through its own resolver.
#[tokio::test]
async fn a_misdeclared_grpc_enhancer_token_fails_create() {
    let message = ToniFactory::create_application_context(GrpcAppModule)
        .await
        .err()
        .expect("a misdeclared enhancer token must fail create")
        .to_string();

    assert!(
        message.contains("not found in registry"),
        "expected an unresolved-enhancer failure, got:\n{message}"
    );
    assert!(
        message.contains("UnregisteredGrpcGuard"),
        "the failure should name the guard, got:\n{message}"
    );
}
