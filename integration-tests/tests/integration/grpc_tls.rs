//! Serving gRPC over TLS.
//!
//! Unlike reflection or a client, this one cannot be reached from outside the
//! adapter: `tonic::transport::Server::tls_config` has to be applied before the
//! routes are added, which happens inside `into_lifecycle`. So the adapter takes
//! the configuration and applies it there — verbatim, since where certificates
//! come from and whether client certificates are demanded are the deployment's
//! questions, not the framework's.
//!
//! It is applied at bind, which is what the second test pins: a certificate the
//! process cannot read fails `app.bind()` rather than the serve task, keeping
//! ADR-0024's refuse-or-none contract.

#![allow(dead_code)]

use std::pin::Pin;
use std::time::Duration;

use futures_util::Stream;
use serial_test::serial;
use toni::ToniFactory;
use toni_macros::{controller, grpc_methods, module, new};
use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};

mod tls_pb {
    tonic::include_proto!("toni_test.orders");
}

use tls_pb::orders_client::OrdersClient;
use tls_pb::orders_server::{Orders, OrdersServer};

/// Minted per run rather than committed, so nothing in the suite expires.
struct SelfSigned {
    cert_pem: String,
    key_pem: String,
}

fn self_signed() -> SelfSigned {
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("a self-signed certificate for localhost");
    SelfSigned {
        cert_pem: issued.cert.pem(),
        key_pem: issued.key_pair.serialize_pem(),
    }
}

#[controller]
pub struct TlsOrders {}

impl TlsOrders {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for TlsOrders {
    async fn create(
        &self,
        request: tonic::Request<tls_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<tls_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(tls_pb::CreateOrderResponse {
            id: 1,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<tls_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<tls_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<tls_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<tls_pb::BulkCreateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<tls_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<tls_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not part of this test"))
    }
}

#[module(controllers: [TlsOrders])]
impl TlsModule {}

/// A caller that trusts the server's certificate completes the handshake and
/// the call, over the same adapter every other test uses in the clear.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_client_that_trusts_the_certificate_is_served() {
    let issued = self_signed();
    let identity = Identity::from_pem(&issued.cert_pem, &issued.key_pem);

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter =
        toni_grpc::GrpcAdapter::new(addr).with_tls(ServerTlsConfig::new().identity(identity));

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(TlsModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    let channel = tonic::transport::Endpoint::from_shared(format!("https://localhost:{port}"))
        .unwrap()
        .tls_config(
            ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(&issued.cert_pem))
                .domain_name("localhost"),
        )
        .expect("the client TLS configuration must be accepted")
        .connect()
        .await
        .expect("the TLS handshake must succeed");

    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        OrdersClient::new(channel).create(tls_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        }),
    )
    .await
    .expect("the call must return")
    .expect("the call must succeed");

    assert_eq!(reply.into_inner().status, "created:keyboard");

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// A certificate the process cannot read is a startup failure. The socket is
/// never opened and the serve task never starts, which is what ADR-0024 asks of
/// every declared transport.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_certificate_that_cannot_be_read_fails_bind() {
    let adapter = toni_grpc::GrpcAdapter::new("127.0.0.1:0".parse().unwrap()).with_tls(
        ServerTlsConfig::new().identity(Identity::from_pem("not a certificate", "not a key")),
    );

    let mut app = ToniFactory::create(TlsModule).await.unwrap();
    app.use_grpc_adapter(adapter).unwrap();

    let failure = app
        .bind()
        .await
        .expect_err("an unreadable identity must refuse the bind");

    // Rendered with its source chain, so the reason is not merely "grpc failed".
    let rendered = format!("{failure:?}");
    assert!(
        rendered.contains("TLS configuration could not be accepted"),
        "the failure must name the certificate as the cause: {rendered}"
    );
}
