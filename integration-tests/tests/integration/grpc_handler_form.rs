//! A gRPC handler reads like a handler on the other three transports.
//!
//! The service writes an inherent impl — `Payload<T>` in, its own reply and
//! its own error out — and `#[grpc_methods(Trait)]` writes the proto trait
//! impl around it: `Request` unwrapped, `Response` wrapped, the error mapped
//! to the code its `kind()` means and parked so the chain sees the type.

#![allow(dead_code)]

use serial_test::serial;
use toni::context::GrpcContext;
use toni::extractors::Payload;
use toni::toni_factory::ToniFactory;
use toni::{async_trait, injectable, module, ErrorKind, GrpcCode, GrpcStatus};
use toni_macros::{controller, grpc_methods, new, use_error_handlers};

mod greeter_pb {
    tonic::include_proto!("toni_test.orders");
}

use greeter_pb::greeter_client::GreeterClient;
use greeter_pb::greeter_server::{Greeter, GreeterServer};

#[derive(Debug)]
struct NoName;

impl std::fmt::Display for NoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a greeting needs a name")
    }
}

impl std::error::Error for NoName {}

impl toni::Error for NoName {
    fn kind(&self) -> ErrorKind {
        ErrorKind::BadRequest
    }
}

/// Claims the domain type, which reaches the chain because the generated
/// method parks it on the execution.
#[injectable]
pub struct NoNameHandler {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for NoNameHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        error.downcast_ref::<NoName>()?;
        Some(GrpcStatus::new(
            GrpcCode::FailedPrecondition,
            "caught:no-name",
        ))
    }
}

#[controller]
pub struct GreeterService {}

#[grpc_methods(greeter_pb::greeter_server::Greeter)]
#[use_error_handlers(NoNameHandler)]
impl GreeterService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The whole handler: no `Request`, no `Response`, no `Status`.
    #[grpc_method]
    async fn greet(
        &self,
        Payload(req): Payload<greeter_pb::GreetRequest>,
        ctx: &GrpcContext,
    ) -> Result<greeter_pb::GreetReply, NoName> {
        if req.name.is_empty() {
            return Err(NoName);
        }
        Ok(greeter_pb::GreetReply {
            message: format!("{} on {}", req.name, ctx.method()),
        })
    }
}

#[module(controllers: [GreeterService], providers: [NoNameHandler])]
impl GreeterModule {}

async fn boot() -> u16 {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new().create_with(GreeterModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.unwrap()
}

async fn client(port: u16) -> GreeterClient<tonic::transport::Channel> {
    GreeterClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    )
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_answers_with_its_own_reply_type() {
    let mut client = client(boot().await).await;

    let reply = client
        .greet(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner();

    // The context param arrived too: the method path is what the caller dialled.
    assert_eq!(reply.message, "ada on toni_test.orders.Greeter/Greet");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_error_reaches_the_chain_with_its_type() {
    let mut client = client(boot().await).await;

    let err = client
        .greet(greeter_pb::GreetRequest {
            name: String::new(),
        })
        .await
        .expect_err("an empty name fails the call");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "caught:no-name");
}
