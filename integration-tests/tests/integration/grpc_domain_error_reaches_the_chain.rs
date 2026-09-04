//! A gRPC handler hands the chain its domain error, not just a status.
//!
//! On every transport a `#[catch(MyError)]` handler downcasts the value the
//! handler returned. tonic's generated method answers with a `Status`, which
//! has no room for the error, so the method the macro writes parks it on the
//! execution on its way out and the chain is handed that.

#![allow(dead_code)]

use crate::common::NotServed;
use serial_test::serial;
use toni::context::GrpcContext;
use toni::extractors::{Inbound, Payload};
use toni::toni_factory::ToniFactory;
use toni::{async_trait, injectable, module, ErrorKind, GrpcCode, GrpcStatus};
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

#[grpc_methods(chain_pb::orders_server::Orders)]
#[use_error_handlers(RestockHandler)]
impl ClaimedGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<chain_pb::CreateOrderRequest>,
    ) -> Result<chain_pb::CreateOrderResponse, OutOfStock> {
        let id = reserve(&req.item)?;
        Ok(chain_pb::CreateOrderResponse {
            id,
            status: "created".into(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<chain_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<chain_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<chain_pb::CreateOrderRequest>,
    ) -> Result<chain_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<chain_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<chain_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
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

#[grpc_methods(chain_pb::orders_server::Orders)]
impl UnclaimedGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<chain_pb::CreateOrderRequest>,
    ) -> Result<chain_pb::CreateOrderResponse, OutOfStock> {
        Err(OutOfStock { item: req.item })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<chain_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<chain_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<chain_pb::CreateOrderRequest>,
    ) -> Result<chain_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<chain_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<chain_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
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
