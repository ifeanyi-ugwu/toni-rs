//! A guard's refusal is a `GuardRejection`, and the error chain sees it.
//!
//! HTTP has always built the typed event and offered it to the chain, so
//! `#[catch(GuardRejection)]` could reshape a refusal there. The other three
//! transports built a transport error instead and returned it, so a catcher
//! never matched — and on WebSocket the refusal reached the caller as nothing
//! at all.
//!
//! One test per remaining transport, each reshaping a refusal into something a
//! catcher chose, which is only possible if the chain was reached.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::NotServed;
use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use toni::async_trait;
use toni::context::{GrpcContext, RpcContext, WsContext};
use toni::errors::GuardRejection;
use toni::extractors::{Inbound, Payload};
use toni::rpc::{RpcData, RpcHandlerOutput, RpcHandlerResult};
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::Guard;
use toni::websocket::{WsHandlerResult, WsMessage};
use toni::{catch, injectable, module, Error, GrpcStatus};
use toni_macros::{
    controller, grpc_methods, message_pattern, new, patterns, subscribe_message, subscriptions,
    use_error_handlers, use_guards, websocket_gateway,
};

use crate::common::TestServer;

mod rejection_pb {
    tonic::include_proto!("toni_test.orders");
}

use rejection_pb::orders_client::OrdersClient;
use rejection_pb::orders_server::{Orders, OrdersServer};

// ── the catchers ───────────────────────────────────────────────────────────

#[catch(GuardRejection)]
async fn ws_catcher(err: &GuardRejection, _ctx: &WsContext) -> WsMessage {
    WsMessage::text(format!("caught:{}", err.message()))
}

#[catch(GuardRejection)]
async fn rpc_catcher(err: &GuardRejection, _ctx: &RpcContext) -> RpcData {
    RpcData::from_serialize(&serde_json::json!({ "caught": err.message() })).unwrap()
}

#[injectable]
pub struct GrpcCatcher {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for GrpcCatcher {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        let rejection = error.downcast_ref::<GuardRejection>()?;
        Some(GrpcStatus::unauthenticated(format!(
            "caught:{}",
            rejection.message()
        )))
    }
}

// ── one refusing guard per transport ───────────────────────────────────────

#[injectable]
pub struct DenyWs {}

#[async_trait]
impl Guard<WsContext> for DenyWs {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        ctx.event() == "connect"
    }
}

#[injectable]
pub struct DenyRpc {}

#[async_trait]
impl Guard<RpcContext> for DenyRpc {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        false
    }
}

#[injectable]
pub struct DenyGrpc {}

#[async_trait]
impl Guard<GrpcContext> for DenyGrpc {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        false
    }
}

// ── WebSocket ──────────────────────────────────────────────────────────────

#[websocket_gateway("/ws-rejection")]
pub struct RejectionGateway {}

#[subscriptions]
#[use_guards(DenyWs)]
impl RejectionGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [RejectionGateway, DenyWs])]
impl WsRejectionModule {}

/// The refusal reaches the caller, and a catcher chose its shape.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_refused_ws_message_is_answered_by_the_chain() {
    let mut factory = ToniFactory::new();
    factory.use_global_ws_error_handler(Arc::new(ws_catcher));
    let server = TestServer::start_with(factory, WsRejectionModule).await;

    let url = format!("ws://127.0.0.1:{}/ws-rejection", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event":"ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("a refused message is answered")
        .expect("the socket stays open")
        .expect("the frame arrives");
    assert_eq!(reply.to_text().unwrap(), "caught:Forbidden");
}

// ── RPC ────────────────────────────────────────────────────────────────────

#[controller]
pub struct RejectionRpcController {}

#[patterns]
#[use_guards(DenyRpc)]
impl RejectionRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rejection.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Single(RpcData::text("unreachable")))
    }
}

#[module(controllers: [RejectionRpcController], providers: [DenyRpc])]
impl RpcRejectionModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_refused_rpc_call_is_answered_by_the_chain() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        factory.use_global_rpc_error_handler(Arc::new(rpc_catcher));
        let mut app = factory.create_with(RpcRejectionModule).await.unwrap();
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
        serde_json::json!({"pattern": "rejection.echo", "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["response"]["caught"], "Forbidden", "reply: {reply}");
}

// ── gRPC ───────────────────────────────────────────────────────────────────

#[controller]
pub struct RejectionGrpcService {}

impl RejectionGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(rejection_pb::orders_server::Orders)]
#[use_guards(DenyGrpc)]
#[use_error_handlers(GrpcCatcher)]
impl RejectionGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<rejection_pb::CreateOrderRequest>,
    ) -> Result<rejection_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<rejection_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<rejection_pb::ProgressEvent, NotServed>>
            + Send
            + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<rejection_pb::CreateOrderRequest>,
    ) -> Result<rejection_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<rejection_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<rejection_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [RejectionGrpcService], providers: [DenyGrpc, GrpcCatcher])]
impl GrpcRejectionModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_refused_grpc_call_is_answered_by_the_chain() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new()
            .create_with(GrpcRejectionModule)
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
        .create(rejection_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        })
        .await
        .expect_err("a refused call must fail");

    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "caught:Forbidden");
}
