//! A domain error answers a gRPC call with its canonical code.
//!
//! Every other transport lifts a `toni::Error` through `?`: the handler
//! returns `Err(OutOfStock)` and the dispatcher renders 409, or the
//! `Conflict` envelope. gRPC fixes the handler's signature to
//! `tonic::Status`, and the orphan rule stops toni converting into a foreign
//! type, so the last hop is explicit — `toni_grpc::to_status`, over the
//! `kind()` mapping the other transports use.

#![allow(dead_code)]

use serial_test::serial;
use toni::toni_factory::ToniFactory;
use toni::{module, ErrorKind};
use toni_macros::{controller, grpc_methods, new};

mod lift_pb {
    tonic::include_proto!("toni_test.orders");
}

use lift_pb::orders_client::OrdersClient;
use lift_pb::orders_server::{Orders, OrdersServer};

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

#[controller]
pub struct LiftGrpcService {}

impl LiftGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for LiftGrpcService {
    async fn create(
        &self,
        request: tonic::Request<lift_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<lift_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Err(toni_grpc::to_status(OutOfStock { item: req.item }))
    }

    type WatchProgressStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<lift_pb::ProgressEvent, tonic::Status>> + Send>,
    >;

    async fn watch_progress(
        &self,
        _request: tonic::Request<lift_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<lift_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<lift_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<lift_pb::ChatMessage, tonic::Status>> + Send>,
    >;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<lift_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [LiftGrpcService])]
impl GrpcErrorLiftModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_domain_error_answers_with_the_code_its_kind_maps_to() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new()
            .create_with(GrpcErrorLiftModule)
            .await
            .unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();

    let mut client = OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    );

    let err = client
        .create(lift_pb::CreateOrderRequest {
            item: "unobtainium".to_string(),
            qty: 1,
        })
        .await
        .expect_err("the domain error must fail the call");

    // `ErrorKind::Conflict` is ABORTED on the wire, and the message is the
    // error's own `Display` — the same text the other transports put in their
    // envelope.
    assert_eq!(err.code(), tonic::Code::Aborted);
    assert_eq!(err.message(), "unobtainium is out of stock");
}
