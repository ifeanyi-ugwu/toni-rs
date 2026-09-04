//! A domain error answers a gRPC call with its canonical code.
//!
//! Every transport renders a `toni::Error` by its `kind()`: `Conflict` is 409
//! on HTTP, the `Conflict` envelope on RPC and WebSocket, and ABORTED here.
//! The handler returns `Err(OutOfStock)`; nothing in it names a status.

#![allow(dead_code)]

use crate::common::NotServed;
use serial_test::serial;
use toni::extractors::{Inbound, Payload};
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

#[grpc_methods(lift_pb::orders_server::Orders)]
impl LiftGrpcService {
    /// The lift the other transports get from `?`: the handler answers with
    /// its own error and the generated method maps the kind to a code.
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<lift_pb::CreateOrderRequest>,
    ) -> Result<lift_pb::CreateOrderResponse, OutOfStock> {
        Err(OutOfStock { item: req.item })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<lift_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<lift_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<lift_pb::CreateOrderRequest>,
    ) -> Result<lift_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<lift_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<lift_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
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
