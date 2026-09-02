//! A gRPC handler can hand the chain its domain error, not just a status.
//!
//! On the other three transports the handler's error type is toni's own, so
//! the value rides the return in an `AppError` variant and a
//! `#[catch(MyError)]` handler downcasts it. tonic fixes the gRPC signature to
//! `Status`, which has no room for the error, so `fail` parks it on the
//! execution and the wrapper hands that to the chain instead.

#![allow(dead_code)]

use serial_test::serial;
use toni::context::GrpcContext;
use toni::toni_factory::ToniFactory;
use toni::{async_trait, injectable, module, ErrorKind, GrpcCode, GrpcStatus};
use toni_grpc::{FailWith, GrpcFail};
use toni_macros::{controller, grpc_methods, new, use_error_handlers};

mod chain_pb {
    tonic::include_proto!("toni_test.orders");
}

use chain_pb::orders_client::OrdersClient;
use chain_pb::orders_server::{Orders, OrdersServer};

#[derive(Debug)]
struct OutOfStock {
    item: String,
}

impl std::fmt::Display for OutOfStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is out of stock", self.item)
    }
}

impl std::error::Error for OutOfStock {}

impl toni::Error for OutOfStock {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Conflict
    }
}

/// Claims the domain type, which is only possible if the value survived the
/// trip through tonic's signature.
#[injectable]
pub struct RestockHandler {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for RestockHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        let out_of_stock = error.downcast_ref::<OutOfStock>()?;
        Some(GrpcStatus::new(
            GrpcCode::FailedPrecondition,
            format!("restock:{}", out_of_stock.item),
        ))
    }
}

fn reserve(item: &str) -> Result<u64, OutOfStock> {
    Err(OutOfStock {
        item: item.to_string(),
    })
}

// ── a service whose failures are claimed by the chain ──────────────────────

#[controller]
pub struct ClaimedGrpcService {}

impl ClaimedGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_error_handlers(RestockHandler)]
impl Orders for ClaimedGrpcService {
    async fn create(
        &self,
        request: tonic::Request<chain_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<chain_pb::CreateOrderResponse>, tonic::Status> {
        let ctx = GrpcContext::of(request.extensions()).expect("a toni-dispatched call");
        let req = request.into_inner();
        let id = reserve(&req.item).fail_with(&ctx)?;
        Ok(tonic::Response::new(chain_pb::CreateOrderResponse {
            id,
            status: "created".into(),
        }))
    }

    type WatchProgressStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<chain_pb::ProgressEvent, tonic::Status>> + Send>,
    >;

    async fn watch_progress(
        &self,
        _request: tonic::Request<chain_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<chain_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<chain_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<chain_pb::ChatMessage, tonic::Status>> + Send>,
    >;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<chain_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [ClaimedGrpcService], providers: [RestockHandler])]
impl ClaimedGrpcModule {}

// ── the same failure with nothing registered to claim it ───────────────────

#[controller]
pub struct UnclaimedGrpcService {}

impl UnclaimedGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for UnclaimedGrpcService {
    async fn create(
        &self,
        request: tonic::Request<chain_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<chain_pb::CreateOrderResponse>, tonic::Status> {
        let ctx = GrpcContext::of(request.extensions()).expect("a toni-dispatched call");
        let req = request.into_inner();
        Err(ctx.fail(OutOfStock { item: req.item }))
    }

    type WatchProgressStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<chain_pb::ProgressEvent, tonic::Status>> + Send>,
    >;

    async fn watch_progress(
        &self,
        _request: tonic::Request<chain_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<chain_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<chain_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<chain_pb::ChatMessage, tonic::Status>> + Send>,
    >;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<chain_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [UnclaimedGrpcService])]
impl UnclaimedGrpcModule {}

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

async fn create_order(port: u16) -> tonic::Status {
    let mut client = OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    );

    client
        .create(chain_pb::CreateOrderRequest {
            item: "unobtainium".to_string(),
            qty: 1,
        })
        .await
        .expect_err("the domain error must fail the call")
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_claims_the_domain_type() {
    let err = create_order(boot(ClaimedGrpcModule).await).await;

    // The handler downcast to `OutOfStock` and rewrote the answer, which it
    // could not have done from the status the error was flattened into.
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "restock:unobtainium");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_unclaimed_failure_keeps_the_status_its_kind_maps_to() {
    let err = create_order(boot(UnclaimedGrpcModule).await).await;

    assert_eq!(err.code(), tonic::Code::Aborted);
    assert_eq!(err.message(), "unobtainium is out of stock");
}
