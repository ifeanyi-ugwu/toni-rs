//! A complete gRPC service wired with DI, a guard, an interceptor, and an
//! error handler.
//!
//! Run:
//!
//! ```
//! cargo run --example grpc_service
//! ```
//!
//! Then in a second terminal:
//!
//! ```
//! # Authorised request — handler runs, counter is bumped, reply comes back.
//! grpcurl -plaintext \
//!     -H 'authorization: Bearer secret-token' \
//!     -d '{"item":"keyboard","qty":3}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     127.0.0.1:50051 toni_examples.orders.Orders/Create
//!
//! # Missing header — the guard rejects with PermissionDenied; the handler
//! # never runs.
//! grpcurl -plaintext \
//!     -d '{"item":"keyboard","qty":3}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     127.0.0.1:50051 toni_examples.orders.Orders/Create
//!
//! # qty=0 — handler returns InvalidArgument with `invalid-qty:0`; the
//! # error handler matches that substring and remaps to FailedPrecondition.
//! grpcurl -plaintext \
//!     -H 'authorization: Bearer secret-token' \
//!     -d '{"item":"keyboard","qty":0}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     127.0.0.1:50051 toni_examples.orders.Orders/Create
//! ```

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use toni::ToniFactory;
use toni_macros::{grpc_methods, grpc_service, injectable, module, new};

mod orders_pb {
    tonic::include_proto!("toni_examples.orders");
}

use orders_pb::orders_server::{Orders, OrdersServer};

// ─── DI'd dependency ────────────────────────────────────────────────────────
//
// Plain provider injected into the service via `#[inject]`. Nothing
// gRPC-specific — works the same as it would on an HTTP controller.

#[injectable]
pub struct OrdersCounter {
    seq: Arc<AtomicU64>,
}
impl OrdersCounter {
    #[new]
    pub fn new() -> Self {
        Self {
            seq: Arc::new(AtomicU64::new(1000)),
        }
    }

    fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }
}

// ─── Guard ──────────────────────────────────────────────────────────────────
//
// The `impl Guard<GrpcContext>` below is the registration — the framework
// detects it. The guard inspects `ctx.metadata()` — for gRPC that's the ASCII
// metadata map the macro builds from the inbound `tonic::Request::metadata()`.
// Returning `false` short-circuits the call with `PermissionDenied`.

#[injectable]
pub struct AuthGuard {}
impl AuthGuard {}

#[toni::async_trait]
impl toni::traits_helpers::Guard<toni::GrpcContext> for AuthGuard {
    async fn can_activate(&self, ctx: &toni::GrpcContext) -> bool {
        matches!(
            ctx.get_metadata("authorization"),
            Some("Bearer secret-token")
        )
    }
}

// ─── Interceptor ────────────────────────────────────────────────────────────
//
// `#[interceptor(grpc)]` registers this as `Interceptor<GrpcContext>`. The
// chain calls `intercept` before the user delegation; calling
// `next.run(ctx).await` proceeds to the next link (and ultimately the
// user method); not calling it short-circuits the call.

#[injectable]
pub struct LoggingInterceptor {}
impl LoggingInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for LoggingInterceptor {
    async fn intercept(
        &self,
        ctx: &mut toni::GrpcContext,
        next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        let method = ctx.method().to_string();
        tracing::info!(target: "grpc_service", method = %method, "before handler");
        next.run(ctx).await;
        tracing::info!(target: "grpc_service", method = %method, "after handler");
    }
}

// ─── Error handler ──────────────────────────────────────────────────────────
//
// `#[error_handler(grpc)]` registers this as
// `ErrorHandler<GrpcContext, GrpcStatus>`. The chain offers it every
// user-returned `Err(Status)` (wrapped as `GrpcStatus`) and every caught
// handler panic. Returning `Some(...)` claims the response with a new
// status; `None` lets the next handler in the chain decide, falling back
// to the original status if none claims.

#[injectable]
pub struct QtyErrorHandler {}
impl QtyErrorHandler {}

#[toni::async_trait]
impl toni::traits_helpers::ErrorHandler<toni::GrpcContext, toni::GrpcStatus> for QtyErrorHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &toni::GrpcContext,
    ) -> Option<toni::GrpcStatus> {
        if error.to_string().contains("invalid-qty") {
            Some(toni::GrpcStatus::new(
                toni::GrpcCode::FailedPrecondition,
                "qty must be positive",
            ))
        } else {
            None
        }
    }
}

// ─── Service ────────────────────────────────────────────────────────────────
//
// `#[grpc_service]` registers the struct as a singleton DI injectable; the
// `#[inject]`-annotated fields are resolved from the module's providers
// list. `#[grpc_methods]` wraps the proto-trait impl with the enhancer
// pipeline declared via the `#[use_*]` attributes — at bind time the
// framework registers the wrapper with tonic so every inbound call
// flows through guards → interceptors → user code → error handlers.

#[grpc_service(pub struct OrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl OrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self {
        Self { counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_guards(AuthGuard)]
#[use_error_handlers(QtyErrorHandler)]
impl Orders for OrdersGrpcService {
    #[use_interceptors(LoggingInterceptor)]
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        if req.qty == 0 {
            // The error handler matches on this substring and remaps to
            // FailedPrecondition. With the handler removed, the original
            // InvalidArgument status would pass through to the wire.
            return Err(tonic::Status::invalid_argument(format!(
                "invalid-qty:{}",
                req.qty
            )));
        }
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            item: req.item.clone(),
            qty: req.qty,
            status: format!("created:{}", req.item),
        }))
    }
}

#[module(providers: [
    OrdersCounter,
    AuthGuard,
    LoggingInterceptor,
    QtyErrorHandler,
    OrdersGrpcService,
])]
struct GrpcExampleModule;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,grpc_service=info".into()),
        )
        .init();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
            let adapter = toni_grpc::GrpcAdapter::new(addr);

            let mut app = ToniFactory::create(GrpcExampleModule::module_definition()).await;
            app.use_grpc_adapter(adapter).unwrap();
            tracing::info!("gRPC server listening on 127.0.0.1:50051");
            app.start().await.expect("server failed to start");
        })
        .await;
}
