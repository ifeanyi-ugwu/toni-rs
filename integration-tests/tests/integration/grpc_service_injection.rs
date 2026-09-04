#![allow(dead_code)]

use futures_util::Stream;
use toni::extractors::{Inbound, Payload};
use toni::*;
use toni_macros::{controller, grpc_methods, new};

use crate::common::NotServed;

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

// `OrdersServer` reads as unused here — `#[grpc_methods]` names it in the code it emits.
use orders_pb::orders_server::{Orders, OrdersServer};

#[controller]
pub struct OrdersGrpcService {}

impl OrdersGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

// The injectable's derived Clone needs the field type Clone; without this impl the test stops at
// that compile error instead of reaching the resolution refusal it pins.
impl Clone for OrdersGrpcService {
    fn clone(&self) -> Self {
        Self {}
    }
}

#[grpc_methods(orders_pb::orders_server::Orders)]
impl OrdersGrpcService {
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
        impl Stream<Item = Result<orders_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<orders_pb::ChatMessage>,
    ) -> Result<
        impl Stream<Item = Result<orders_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[injectable]
pub struct OrdersReporter {
    #[inject]
    service: OrdersGrpcService,
}

#[module(controllers: [OrdersGrpcService], providers: [OrdersReporter])]
impl AppModule {}

/// A gRPC service is a dispatch target: it is reached by its transport and nothing may hold it.
/// Declared in `controllers:`, its token is not in the provider store, so injecting it into an
/// ordinary provider fails resolution at init.
///
/// The other half of the refusal is not reachable from here: listing a dispatch target in
/// `providers:` does not compile, because the macro emits no provider factory for one.
#[tokio::test]
async fn a_grpc_service_is_not_resolvable_as_a_dependency() {
    let message = ToniFactory::create_application_context(AppModule)
        .await
        .err()
        .expect("an injected dispatch target must fail initialization")
        .to_string();

    assert!(
        message.contains("Dependency not found"),
        "expected an unresolved-dependency failure, got:\n{message}"
    );
    assert!(
        message.contains("OrdersGrpcService"),
        "the failure should name the service, got:\n{message}"
    );
}
