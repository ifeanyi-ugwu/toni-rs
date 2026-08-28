#![allow(dead_code)]

use std::pin::Pin;

use futures_util::Stream;
use toni::*;
use toni_macros::{grpc_methods, grpc_service, new};

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

// `OrdersServer` reads as unused here — `#[grpc_methods]` names it in the code it emits.
use orders_pb::orders_server::{Orders, OrdersServer};

#[grpc_service(pub struct OrdersGrpcService {})]
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

#[grpc_methods]
#[tonic::async_trait]
impl Orders for OrdersGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Ok(tonic::Response::new(
            Box::pin(futures_util::stream::empty()),
        ))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Ok(tonic::Response::new(
            Box::pin(futures_util::stream::empty()),
        ))
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
