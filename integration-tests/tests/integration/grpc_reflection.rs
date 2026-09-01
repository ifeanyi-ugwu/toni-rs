//! Server reflection is a service the adapter is handed, not a feature it owns.
//!
//! `GrpcAdapter::add_service` takes anything satisfying tonic's service
//! contract, and `tonic-reflection` produces exactly that. So a server that
//! `grpcurl` can explore without a local `.proto` needs no framework support —
//! the descriptor set comes out of `build.rs`, and the service is registered
//! beside the DI-discovered ones.
//!
//! What this pins is that the two coexist: reflection lists the service the
//! framework registered through `#[grpc_methods]`, which it can only do if both
//! reached the same route set.

#![allow(dead_code)]

use std::pin::Pin;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serial_test::serial;
use toni::ToniFactory;
use toni_macros::{controller, grpc_methods, module, new};
use tonic_reflection::pb::v1::server_reflection_client::ServerReflectionClient;
use tonic_reflection::pb::v1::server_reflection_request::MessageRequest;
use tonic_reflection::pb::v1::server_reflection_response::MessageResponse;
use tonic_reflection::pb::v1::ServerReflectionRequest;

mod reflect_pb {
    tonic::include_proto!("toni_test.orders");
}

use reflect_pb::orders_server::{Orders, OrdersServer};

/// Written by `build.rs` through `file_descriptor_set_path`. This is the whole
/// build-side cost of reflection.
const ORDERS_DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/orders_descriptor.bin"));

#[controller]
pub struct ReflectedOrders {}

impl ReflectedOrders {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for ReflectedOrders {
    async fn create(
        &self,
        request: tonic::Request<reflect_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<reflect_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(reflect_pb::CreateOrderResponse {
            id: 1,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<reflect_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<reflect_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<reflect_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<reflect_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<reflect_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<reflect_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [ReflectedOrders])]
impl ReflectionModule {}

async fn boot() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(ORDERS_DESCRIPTOR)
        .build_v1()
        .expect("the descriptor set must build a reflection service");
    let adapter = toni_grpc::GrpcAdapter::new(addr).add_service(reflection);

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ReflectionModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("grpc must bind").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

/// A client with no `.proto` asks the server what it serves, and is told about
/// the service `#[grpc_methods]` registered.
#[serial]
#[tokio_localset_test::localset_test]
async fn reflection_lists_the_services_the_framework_registered() {
    let (port, shutdown) = boot().await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .expect("gRPC connect should succeed");
    let mut client = ServerReflectionClient::new(channel);

    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(MessageRequest::ListServices(String::new())),
    };
    let mut replies = client
        .server_reflection_info(tokio_stream::iter(vec![request]))
        .await
        .expect("the reflection service must answer")
        .into_inner();

    let reply = tokio::time::timeout(Duration::from_secs(2), replies.next())
        .await
        .expect("a reply must arrive")
        .expect("the stream must yield")
        .expect("the reply must be Ok");

    let listed = match reply.message_response {
        Some(MessageResponse::ListServicesResponse(list)) => {
            list.service.into_iter().map(|s| s.name).collect::<Vec<_>>()
        }
        other => panic!("expected a service list, got {other:?}"),
    };

    assert!(
        listed.iter().any(|name| name == "toni_test.orders.Orders"),
        "the DI-registered service must be discoverable: {listed:?}"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}
