//! A panic anywhere the chain can reach is a `PanicRecovered` the chain sees.
//!
//! Guards, handlers and renderers were already covered. The two segments left
//! out were a panicking HTTP `Middleware`, which escaped the dispatcher
//! entirely, and a gRPC interceptor, whose event was built deep in the link
//! chain and turned straight into a status.
//!
//! Each test reshapes the panic into something a catcher chose, which is only
//! possible if the chain was reached, and reads the segment back off the event.

#![allow(dead_code)]

use std::sync::Arc;

use crate::common::NotServed;
use serial_test::serial;
use toni::async_trait;
use toni::context::{GrpcContext, HttpContext};
use toni::errors::PanicRecovered;
use toni::extractors::{Inbound, Payload};
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::middleware::{Middleware, MiddlewareResult, NextHandle};
use toni::traits_helpers::MiddlewareConsumer;
use toni::traits_helpers::{Interceptor, InterceptorNext};
use toni::{catch, controller, get, injectable, module, routes, GrpcStatus, HttpResponse};
use toni_macros::{grpc_methods, new, use_error_handlers, use_interceptors};

use crate::common::TestServer;

mod pipeline_pb {
    tonic::include_proto!("toni_test.orders");
}

use pipeline_pb::orders_client::OrdersClient;
use pipeline_pb::orders_server::{Orders, OrdersServer};

// ── HTTP: a panicking middleware ───────────────────────────────────────────

#[catch(PanicRecovered)]
async fn http_panic_catcher(err: &PanicRecovered, _ctx: &HttpContext) -> HttpResponse {
    HttpResponse::builder()
        .status(418)
        .json(serde_json::json!({ "caught": err.during.as_str() }))
        .build()
}

struct PanickingMiddleware;

#[async_trait]
impl Middleware for PanickingMiddleware {
    async fn handle(&self, _next: NextHandle) -> MiddlewareResult {
        panic!("middleware kaboom");
    }
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_panicking_middleware_is_answered_by_the_chain() {
    #[controller("/")]
    pub struct MiddlewarePanicController {}

    #[routes]
    impl MiddlewarePanicController {
        #[get("/ping")]
        fn ping(&self) -> toni::Body {
            toni::Body::text("unreachable")
        }
    }

    #[module(controllers: [MiddlewarePanicController])]
    impl MiddlewarePanicModule {
        fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
            consumer.apply(PanickingMiddleware).for_routes(vec!["/*"]);
        }
    }

    let mut factory = ToniFactory::new();
    factory.use_global_http_error_handler(Arc::new(http_panic_catcher));

    let server = TestServer::start_with(factory, MiddlewarePanicModule).await;
    let resp = server
        .client()
        .get(server.url("/ping"))
        .send()
        .await
        .expect("a panicking middleware must answer, not drop the connection");

    assert_eq!(resp.status().as_u16(), 418);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["caught"], "middleware", "body: {body}");
}

// ── gRPC: a panicking interceptor ──────────────────────────────────────────

#[injectable]
pub struct GrpcPipelineCatcher {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for GrpcPipelineCatcher {
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

#[injectable]
pub struct PanickingGrpcPipelineInterceptor {}

#[async_trait]
impl Interceptor<GrpcContext, Result<(), GrpcStatus>> for PanickingGrpcPipelineInterceptor {
    async fn intercept(
        &self,
        _ctx: &GrpcContext,
        _next: Box<dyn InterceptorNext<GrpcContext, Result<(), GrpcStatus>>>,
    ) -> Result<(), GrpcStatus> {
        panic!("grpc interceptor kaboom");
    }
}

#[controller]
pub struct InterceptorPanicPipelineService {}

impl InterceptorPanicPipelineService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(pipeline_pb::orders_server::Orders)]
#[use_interceptors(PanickingGrpcPipelineInterceptor)]
#[use_error_handlers(GrpcPipelineCatcher)]
impl InterceptorPanicPipelineService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<pipeline_pb::CreateOrderRequest>,
    ) -> Result<pipeline_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<pipeline_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<pipeline_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<pipeline_pb::CreateOrderRequest>,
    ) -> Result<pipeline_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<pipeline_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<pipeline_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(
    controllers: [InterceptorPanicPipelineService],
    providers: [PanickingGrpcPipelineInterceptor, GrpcPipelineCatcher],
)]
impl GrpcInterceptorPanicModule {}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_panicking_grpc_interceptor_is_answered_by_the_chain() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new()
            .create_with(GrpcInterceptorPanicModule)
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
        .create(pipeline_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        })
        .await
        .expect_err("a panicking interceptor must fail the call");

    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "caught:middleware");
}
