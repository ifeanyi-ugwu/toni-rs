//! `bind()` must refuse an application that would come up serving less than it declares.
//!
//! Every case here starts an application whose HTTP half is perfectly healthy. Before the
//! refusal it would have returned `Ok`, logged the failure at `error` level, and left a process
//! that answers HTTP while its other transport is absent — a state `BoundAdapters` cannot even
//! report, since `rpc: None` also means "this transport has no address".

use std::net::TcpListener;

use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError};
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{module, BindError, ToniFactory};
use toni_axum::AxumAdapter;
use toni_macros::{
    message_pattern, new, patterns, rpc_controller, subscribe_message, subscriptions,
    websocket_gateway,
};
use toni_tcp::TcpAdapter;

#[rpc_controller]
pub struct EchoController {}

#[patterns]
impl EchoController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("echo")]
    async fn echo(&self, data: RpcData, _ctx: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [EchoController])]
struct RpcModule;

/// Declares its own port, so it needs a `WebSocketAdapter` rather than the HTTP listener.
#[websocket_gateway("/events", port = 19310)]
pub struct EventsGateway {}

#[subscriptions]
impl EventsGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [EventsGateway])]
struct SeparatePortModule;

#[module]
struct BareModule;

/// A port number nothing is listening on. Racy in principle; the window between the probe
/// closing and the adapter binding is a few microseconds on a loopback interface.
fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    probe.local_addr().unwrap().port()
}

#[tokio_localset_test::localset_test]
async fn an_rpc_adapter_that_cannot_bind_fails_the_bind() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = occupied.local_addr().unwrap().port();

    let mut app = ToniFactory::create(RpcModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.use_rpc_adapter(TcpAdapter::new("127.0.0.1", taken))
        .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("a taken RPC port must fail bind");

    assert!(
        matches!(
            err,
            BindError::Adapter {
                transport: "rpc",
                ..
            }
        ),
        "expected an rpc adapter failure, got: {err}"
    );
}

#[tokio_localset_test::localset_test]
async fn a_failed_bind_releases_the_sockets_it_already_took() {
    let rpc_port = free_port();
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_grpc = occupied.local_addr().unwrap();

    let mut app = ToniFactory::create(RpcModule).await;
    // RPC binds before gRPC, so its socket is live when gRPC fails.
    app.use_rpc_adapter(TcpAdapter::new("127.0.0.1", rpc_port))
        .unwrap();
    app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(taken_grpc))
        .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("a taken gRPC port must fail bind");
    assert!(
        matches!(
            err,
            BindError::Adapter {
                transport: "grpc",
                ..
            }
        ),
        "expected a grpc adapter failure, got: {err}"
    );

    TcpListener::bind(("127.0.0.1", rpc_port))
        .expect("the RPC socket bound before the failure should have been released");
}

#[tokio_localset_test::localset_test]
async fn rpc_patterns_with_no_rpc_adapter_are_refused() {
    let mut app = ToniFactory::create(RpcModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("declared patterns with no transport must fail bind");

    let message = err.to_string();
    assert!(
        message.contains("use_rpc_adapter"),
        "the refusal should say what is missing, got: {message}"
    );
}

#[tokio_localset_test::localset_test]
async fn a_separate_port_gateway_with_no_websocket_adapter_is_refused() {
    let mut app = ToniFactory::create(SeparatePortModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("a gateway on its own port with no adapter must fail bind");

    let message = err.to_string();
    assert!(
        message.contains("use_websocket_adapter"),
        "the refusal should say what is missing, got: {message}"
    );
}

#[tokio_localset_test::localset_test]
async fn a_websocket_listener_no_gateway_declares_is_refused() {
    let orphan = TcpListener::bind("127.0.0.1:0").unwrap();

    let mut app = ToniFactory::create(BareModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    // 19311 is declared by no gateway, so nothing would ever accept on this socket.
    app.use_websocket_listener(19311, orphan).unwrap();

    let err = app
        .bind()
        .await
        .expect_err("a socket no gateway claims must fail bind");

    let message = err.to_string();
    assert!(
        message.contains("19311"),
        "the refusal should name the unclaimed port, got: {message}"
    );
}
