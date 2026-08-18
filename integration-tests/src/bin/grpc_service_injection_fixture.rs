//! Injects a gRPC service into an ordinary provider. A service is reached by its transport and is
//! not a dependency, so the injector refuses it and `create_application_context` logs the
//! diagnostic and exits with status 1.
//!
//! A subprocess is required because the only public trigger path calls `std::process::exit`.

#![allow(dead_code)]

use std::pin::Pin;

use futures_util::Stream;
use toni::*;
use toni_macros::{grpc_methods, grpc_service, new};
use tracing_subscriber::{fmt, EnvFilter};

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

use orders_pb::orders_server::{Orders, OrdersServer};

#[grpc_service(pub struct OrdersGrpcService {})]
impl OrdersGrpcService {
    #[new]
    pub fn new() -> Self {
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

#[module(providers: [OrdersGrpcService, OrdersReporter])]
impl AppModule {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    fmt()
        .with_env_filter(EnvFilter::new("error"))
        .with_target(false)
        .init();

    let _ctx = ToniFactory::create_application_context(AppModule).await;
}
