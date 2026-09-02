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
//! # qty=0 — the handler fails with an `InvalidQty` domain error; the error
//! # handler downcasts it and remaps to FailedPrecondition.
//! grpcurl -plaintext \
//!     -H 'authorization: Bearer secret-token' \
//!     -d '{"item":"keyboard","qty":0}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     127.0.0.1:50051 toni_examples.orders.Orders/Create
//!
//! # Out of stock — a domain error carrying `ErrorKind::Conflict`, lifted by
//! # `toni_grpc::to_status`, which answers ABORTED.
//! grpcurl -plaintext \
//!     -H 'authorization: Bearer secret-token' \
//!     -d '{"item":"unobtainium","qty":1}' \
//!     -proto examples/proto/orders.proto \
//!     -import-path examples/proto \
//!     127.0.0.1:50051 toni_examples.orders.Orders/Create
//! ```

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use toni::ToniFactory;
use toni_grpc::GrpcFail;
use toni_macros::{controller, grpc_methods, injectable, module, new};

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

// ─── Domain error ───────────────────────────────────────────────────────────
//
// The same `toni::Error` a handler on any other transport would return. Its
// `kind()` decides the wire shape everywhere: 409 on HTTP, `Conflict` in the
// RPC and WebSocket envelopes, ABORTED here. A gRPC handler answers with
// `tonic::Status` by signature, so the last hop is `toni_grpc::to_status`
// rather than `?` — the orphan rule keeps toni from writing that conversion.

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
    fn kind(&self) -> toni::ErrorKind {
        toni::ErrorKind::Conflict
    }
}

#[derive(Debug)]
struct InvalidQty {
    qty: u32,
}

impl std::fmt::Display for InvalidQty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "qty must be positive, got {}", self.qty)
    }
}

impl std::error::Error for InvalidQty {}

impl toni::Error for InvalidQty {
    fn kind(&self) -> toni::ErrorKind {
        toni::ErrorKind::BadRequest
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
        matches!(ctx.header("authorization"), Some("Bearer secret-token"))
    }
}

// ─── Interceptor ────────────────────────────────────────────────────────────
//
// `#[interceptor(grpc)]` registers this as `Interceptor<GrpcContext, GrpcHandlerResult>`. The
// chain calls `intercept` before the user delegation; calling
// `next.run(ctx).await` proceeds to the next link (and ultimately the
// user method); not calling it short-circuits the call.

#[injectable]
pub struct LoggingInterceptor {}
impl LoggingInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext, toni::GrpcHandlerResult>
    for LoggingInterceptor
{
    async fn intercept(
        &self,
        ctx: &toni::GrpcContext,
        next: Box<
            dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext, toni::GrpcHandlerResult>,
        >,
    ) -> toni::GrpcHandlerResult {
        let method = ctx.method().to_string();
        tracing::info!(target: "grpc_service", method = %method, "before handler");
        let answer = next.run(ctx).await;
        tracing::info!(target: "grpc_service", method = %method, "after handler");
        answer
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
        // The handler failed through `ctx.fail`, which parks the domain error
        // on the execution, so the chain is handed `InvalidQty` itself rather
        // than the status it maps to. Without that, this would be matching on
        // a substring of the message.
        let invalid = error.downcast_ref::<InvalidQty>()?;
        Some(toni::GrpcStatus::new(
            toni::GrpcCode::FailedPrecondition,
            format!("qty must be positive, got {}", invalid.qty),
        ))
    }
}

// ─── Service ────────────────────────────────────────────────────────────────
//
// `#[controller]` declares the service as a dispatch target; the
// `#[inject]`-annotated fields are resolved from the module's providers
// list. `#[grpc_methods]` wraps the proto-trait impl with the enhancer
// pipeline declared via the `#[use_*]` attributes — at bind time the
// framework registers the wrapper with tonic so every inbound call
// flows through guards → interceptors → user code → error handlers.

#[controller]
pub struct OrdersGrpcService {
    #[inject]
    counter: OrdersCounter,
}

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
        let ctx = toni::GrpcContext::of(request.extensions()).expect("a toni-dispatched call");
        let req = request.into_inner();
        if req.qty == 0 {
            // `fail` answers with the status `BadRequest` maps to and leaves
            // the error where the chain can still see its type. With the
            // handler removed, that InvalidArgument reaches the wire.
            return Err(ctx.fail(InvalidQty { qty: req.qty }));
        }
        if req.item == "unobtainium" {
            // No chain handler claims this one, so the mapped code is the
            // answer: `Conflict` is ABORTED.
            return Err(ctx.fail(OutOfStock {
                item: req.item.clone(),
            }));
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

#[module(controllers: [OrdersGrpcService], providers: [OrdersCounter, AuthGuard, LoggingInterceptor, QtyErrorHandler])]
struct GrpcExampleModule;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
            let adapter = toni_grpc::GrpcAdapter::new(addr);

            let mut app = ToniFactory::create(GrpcExampleModule).await.unwrap();
            app.use_grpc_adapter(adapter).unwrap();
            tracing::info!("gRPC server listening on 127.0.0.1:50051");
            app.start().await.expect("server failed to start");
        })
        .await;
}
