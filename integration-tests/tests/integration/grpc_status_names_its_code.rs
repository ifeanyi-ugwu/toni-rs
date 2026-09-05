//! A handler names a gRPC code the kind table cannot reach.
//!
//! `grpc_code` maps eleven `ErrorKind`s onto the canonical codes.
//! `FailedPrecondition` and `OutOfRange` are outside it, and reaching one used
//! to mean registering a chain handler to claim the error and answer with the
//! status. `GrpcStatus` is a `toni::Error`, so a handler returns one directly.

#![allow(dead_code)]

use serial_test::serial;
use toni::context::GrpcContext;
use toni::extractors::{Inbound, Payload};
use toni::toni_factory::ToniFactory;
use toni::{async_trait, injectable, module, ErrorKind, GrpcCode, GrpcStatus};
use toni_macros::{controller, grpc_methods, new, use_error_handlers};

use crate::common::NotServed;

mod named_pb {
    tonic::include_proto!("toni_test.orders");
}

use named_pb::orders_client::OrdersClient;
use named_pb::orders_server::{Orders, OrdersServer};

/// A kind maps this to `Aborted`; the handler answers `OutOfRange` instead, so
/// a green assertion on the wire code says the named one was not re-derived.
#[derive(Debug)]
struct WindowClosed;

impl std::fmt::Display for WindowClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the booking window is closed")
    }
}

impl std::error::Error for WindowClosed {}

impl toni::Error for WindowClosed {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Conflict
    }
}

/// Claims `WindowClosed`, which it can only see if the error travelled on the
/// status the handler named rather than being flattened out of it.
#[injectable]
pub struct ReopenHandler {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for ReopenHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        error.downcast_ref::<WindowClosed>()?;
        Some(GrpcStatus::new(GrpcCode::Unavailable, "try again at 09:00"))
    }
}

#[controller]
pub struct NamedCodeService {}

#[grpc_methods(named_pb::orders_server::Orders)]
impl NamedCodeService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<named_pb::CreateOrderRequest>,
    ) -> Result<named_pb::CreateOrderResponse, GrpcStatus> {
        if req.item == "late" {
            return Err(GrpcStatus::new(
                GrpcCode::FailedPrecondition,
                "the booking window is closed",
            ));
        }
        // The code is named and the error rides along, so a chain handler can
        // still match the type.
        Err(GrpcStatus::new(GrpcCode::OutOfRange, "past the last slot").caused_by(WindowClosed))
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<named_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<named_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<named_pb::CreateOrderRequest>,
    ) -> Result<named_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<named_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<named_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [NamedCodeService])]
impl NamedCodeModule {}

#[controller]
pub struct ClaimedNamedCodeService {}

#[grpc_methods(named_pb::orders_server::Orders)]
#[use_error_handlers(ReopenHandler)]
impl ClaimedNamedCodeService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<named_pb::CreateOrderRequest>,
    ) -> Result<named_pb::CreateOrderResponse, GrpcStatus> {
        Err(GrpcStatus::new(GrpcCode::OutOfRange, "past the last slot").caused_by(WindowClosed))
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<named_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<named_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<named_pb::CreateOrderRequest>,
    ) -> Result<named_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<named_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<named_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [ClaimedNamedCodeService], providers: [ReopenHandler])]
impl ClaimedNamedCodeModule {}

async fn boot(module: impl toni::ModuleMetadata + 'static) -> u16 {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new().create_with(module).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.unwrap()
}

async fn create(port: u16, item: &str) -> tonic::Status {
    let mut client = OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    );

    client
        .create(named_pb::CreateOrderRequest {
            item: item.to_string(),
            qty: 1,
        })
        .await
        .expect_err("the handler fails every call")
}

/// No `ErrorKind` maps to `FailedPrecondition`, so deriving the code from the
/// status's own `kind()` would answer `Internal`.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_answers_with_a_code_no_kind_reaches() {
    let err = create(boot(NamedCodeModule).await, "late").await;

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "the booking window is closed");
}

/// Nothing is registered to claim it, so the named code goes to the wire even
/// though the carried error's kind maps elsewhere.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_carried_error_does_not_overwrite_the_named_code() {
    let err = create(boot(NamedCodeModule).await, "early").await;

    assert_eq!(err.code(), tonic::Code::OutOfRange);
    assert_eq!(err.message(), "past the last slot");
}

/// The chain is offered the carried error rather than the status it rode on.
#[serial]
#[tokio_localset_test::localset_test]
async fn the_chain_sees_the_error_a_named_status_carries() {
    let err = create(boot(ClaimedNamedCodeModule).await, "early").await;

    assert_eq!(err.code(), tonic::Code::Unavailable);
    assert_eq!(err.message(), "try again at 09:00");
}
