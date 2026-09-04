//! A guard that panics is a `PanicRecovered`, and the error chain sees it.
//!
//! HTTP and WebSocket route both guard exits — the refusal and the panic —
//! through the chain. RPC and gRPC routed only the refusal and returned the
//! panic straight from the guard loop, so a catcher never matched and the
//! caller was told its credentials were refused for what is a server bug.
//!
//! One test per transport, each reshaping the panic into something a catcher
//! chose, which is only possible if the chain was reached. The segment the
//! catcher reads back is `guard`, so the typed event arrives whole.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::NotServed;
use serial_test::serial;
use toni::async_trait;
use toni::context::{GrpcContext, RpcContext};
use toni::errors::PanicRecovered;
use toni::extractors::{Inbound, Payload};
use toni::rpc::{RpcData, RpcHandlerOutput, RpcHandlerResult};
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::Guard;
use toni::{catch, injectable, module, GrpcStatus};
use toni_macros::{
    controller, grpc_methods, message_pattern, new, patterns, use_error_handlers, use_guards,
};

mod panic_pb {
    tonic::include_proto!("toni_test.orders");
}

use panic_pb::orders_client::OrdersClient;
use panic_pb::orders_server::{Orders, OrdersServer};

// ── the catchers ───────────────────────────────────────────────────────────

#[catch(PanicRecovered)]
async fn rpc_panic_catcher(err: &PanicRecovered, _ctx: &RpcContext) -> RpcData {
    RpcData::from_serialize(&serde_json::json!({ "caught": err.during.as_str() })).unwrap()
}

#[injectable]
pub struct GrpcPanicCatcher {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for GrpcPanicCatcher {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        let panic = error.downcast_ref::<PanicRecovered>()?;
        Some(GrpcStatus::unauthenticated(format!(
            "caught:{}",
            panic.during.as_str()
        )))
    }
}

// ── one panicking guard per transport ──────────────────────────────────────

#[injectable]
pub struct PanicRpcGuard {}

#[async_trait]
impl Guard<RpcContext> for PanicRpcGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        panic!("rpc guard kaboom");
    }
}

#[injectable]
pub struct PanicGrpcGuard {}

#[async_trait]
impl Guard<GrpcContext> for PanicGrpcGuard {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        panic!("grpc guard kaboom");
    }
}

// ── RPC ────────────────────────────────────────────────────────────────────

#[controller]
pub struct PanicGuardRpcController {}

#[patterns]
#[use_guards(PanicRpcGuard)]
impl PanicGuardRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("guard.panic.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Single(RpcData::text("unreachable")))
    }
}

#[module(controllers: [PanicGuardRpcController], providers: [PanicRpcGuard])]
impl RpcGuardPanicEventModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_panicking_rpc_guard_is_answered_by_the_chain() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        factory.use_global_rpc_error_handler(Arc::new(rpc_panic_catcher));
        let mut app = factory.create_with(RpcGuardPanicEventModule).await.unwrap();
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame =
        serde_json::json!({"pattern": "guard.panic.echo", "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["response"]["caught"], "guard", "reply: {reply}");
}

// ── gRPC ───────────────────────────────────────────────────────────────────

#[controller]
pub struct PanicGuardGrpcService {}

impl PanicGuardGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(panic_pb::orders_server::Orders)]
#[use_guards(PanicGrpcGuard)]
#[use_error_handlers(GrpcPanicCatcher)]
impl PanicGuardGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<panic_pb::CreateOrderRequest>,
    ) -> Result<panic_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<panic_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<panic_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<panic_pb::CreateOrderRequest>,
    ) -> Result<panic_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<panic_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<panic_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [PanicGuardGrpcService], providers: [PanicGrpcGuard, GrpcPanicCatcher])]
impl GrpcGuardPanicEventModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_panicking_grpc_guard_is_answered_by_the_chain() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new()
            .create_with(GrpcGuardPanicEventModule)
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
        .create(panic_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        })
        .await
        .expect_err("a panicking guard must fail the call");

    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "caught:guard");
}
