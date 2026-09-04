//! Global gRPC enhancers, registered on the factory rather than named on a
//! service.
//!
//! HTTP, RPC and WebSocket each take guards, interceptors and an error handler
//! that apply to every handler of that transport. gRPC took none, so a policy
//! every service must obey had to be repeated on every service.
//!
//! Ordering is the same as elsewhere: global, then the service's, then the
//! method's.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::common::NotServed;
use serial_test::serial;
use toni::context::GrpcContext;
use toni::extractors::{Inbound, Payload};
use toni::traits_helpers::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use toni::ToniFactory;
use toni::{GrpcHandlerResult, GrpcStatus};
use toni_macros::{controller, grpc_methods, injectable, module, new, use_guards};

mod globals_pb {
    tonic::include_proto!("toni_test.orders");
}

use globals_pb::orders_client::OrdersClient;
use globals_pb::orders_server::{Orders, OrdersServer};

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: &str) {
    SEEN.lock().unwrap().push(what.to_string());
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

// ── the enhancers ──────────────────────────────────────────────────────────

struct GlobalGuard;

#[toni::async_trait]
impl Guard<GrpcContext> for GlobalGuard {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        record("global:guard");
        true
    }
}

struct DenyingGlobalGuard;

#[toni::async_trait]
impl Guard<GrpcContext> for DenyingGlobalGuard {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        record("global:deny");
        false
    }
}

struct GlobalInterceptor;

#[toni::async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for GlobalInterceptor {
    async fn intercept(
        &self,
        ctx: &GrpcContext,
        next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        record("global:before");
        let answer = next.run(ctx).await;
        record("global:after");
        answer
    }
}

/// Claims anything the service left unhandled, so a caller sees one shape
/// whatever the method did.
struct GlobalErrorHandler;

#[toni::async_trait]
impl ErrorHandler<GrpcContext, GrpcStatus> for GlobalErrorHandler {
    async fn handle_error(&self, _error: ChainError<'_>, _ctx: &GrpcContext) -> Option<GrpcStatus> {
        record("global:error_handler");
        Some(GrpcStatus::permission_denied("claimed globally"))
    }
}

#[injectable]
pub struct ServiceGuard {}

#[toni::async_trait]
impl Guard<GrpcContext> for ServiceGuard {
    async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
        record("service:guard");
        true
    }
}

// ── the service ────────────────────────────────────────────────────────────

#[controller]
pub struct GlobalsGrpcService {}

impl GlobalsGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

/// `BadRequest` is INVALID_ARGUMENT on the wire, which is what this handler
/// answered with before it could name its own error.
#[derive(Debug)]
struct InvalidQty;

impl std::fmt::Display for InvalidQty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "qty must be positive")
    }
}

impl std::error::Error for InvalidQty {}

impl toni::Error for InvalidQty {
    fn kind(&self) -> toni::ErrorKind {
        toni::ErrorKind::BadRequest
    }
}

#[grpc_methods(globals_pb::orders_server::Orders)]
#[use_guards(ServiceGuard)]
impl GlobalsGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<globals_pb::CreateOrderRequest>,
    ) -> Result<globals_pb::CreateOrderResponse, InvalidQty> {
        record("handler");
        if req.qty == 0 {
            return Err(InvalidQty);
        }
        Ok(globals_pb::CreateOrderResponse {
            id: 1,
            status: "created".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<globals_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<globals_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<globals_pb::CreateOrderRequest>,
    ) -> Result<globals_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<globals_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<globals_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [GlobalsGrpcService], providers: [ServiceGuard])]
impl GlobalsGrpcModule {}

// ── harness ────────────────────────────────────────────────────────────────

async fn boot<F>(configure: F) -> (u16, toni::ShutdownHandle)
where
    F: FnOnce(&mut ToniFactory) + Send + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(GlobalsGrpcModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("grpc must bind").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    )
}

fn order(item: &str, qty: u32) -> globals_pb::CreateOrderRequest {
    globals_pb::CreateOrderRequest {
        item: item.to_string(),
        qty,
    }
}

async fn stop(shutdown: toni::ShutdownHandle) {
    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

// ── tests ──────────────────────────────────────────────────────────────────

/// A guard the service never names still runs, and runs first.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_guard_runs_ahead_of_the_service_s_own() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_guards(Arc::new(GlobalGuard));
    })
    .await;
    let mut client = connect(port).await;

    client
        .create(order("keyboard", 1))
        .await
        .expect("call must succeed");

    assert_eq!(seen(), vec!["global:guard", "service:guard", "handler"]);
    stop(shutdown).await;
}

/// And rejecting from there stops the call before the service's guard is asked.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_guard_rejecting_stops_the_call() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_guards(Arc::new(DenyingGlobalGuard));
    })
    .await;
    let mut client = connect(port).await;

    let err = client
        .create(order("keyboard", 1))
        .await
        .expect_err("a rejecting guard must fail the call");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    assert_eq!(seen(), vec!["global:deny"]);
    stop(shutdown).await;
}

/// An interceptor registered globally wraps the whole chain below it.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_interceptor_wraps_every_method() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_interceptors(Arc::new(GlobalInterceptor));
    })
    .await;
    let mut client = connect(port).await;

    client
        .create(order("keyboard", 1))
        .await
        .expect("call must succeed");

    assert_eq!(
        seen(),
        vec!["service:guard", "global:before", "handler", "global:after"],
        "guards answer before the interceptor chain is entered"
    );
    stop(shutdown).await;
}

/// A handler's own `Err` is offered to the global handler, which reshapes it.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_error_handler_claims_what_the_service_leaves() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_error_handler(Arc::new(GlobalErrorHandler));
    })
    .await;
    let mut client = connect(port).await;

    let err = client
        .create(order("ignored", 0))
        .await
        .expect_err("qty=0 must fail");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert_eq!(err.message(), "claimed globally");

    assert!(seen().contains(&"global:error_handler".to_string()));
    stop(shutdown).await;
}
