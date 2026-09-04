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
// `kind()` decides the wire shape everywhere: 400 and 409 on HTTP, `BadRequest`
// and `Conflict` in the RPC and WebSocket envelopes, INVALID_ARGUMENT and
// ABORTED here.

#[derive(Debug, toni::Error)]
enum OrderError {
    #[error_kind(BadRequest)]
    InvalidQty { qty: u32 },

    #[error_kind(Conflict)]
    OutOfStock { item: String },
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidQty { qty } => write!(f, "qty must be positive, got {qty}"),
            Self::OutOfStock { item } => write!(f, "{item} is out of stock"),
        }
    }
}

impl std::error::Error for OrderError {}

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
// A provider implementing `Interceptor<GrpcContext, GrpcHandlerResult>` is
// registered as one by that impl. The chain calls `intercept` before the
// handler; `next.run(ctx).await` proceeds to the next link and ultimately to
// the handler, and not calling it short-circuits the call.

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
// A provider implementing `ErrorHandler<GrpcContext, GrpcStatus>` is
// registered as one by that impl. The chain offers it every error a handler
// returned and every caught panic. Returning `Some(...)` claims the answer with
// a new status; `None` lets the next handler decide, falling back to the status
// the error's kind maps to if none claims.

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
        // The chain is handed the handler's own error, so this matches a
        // variant rather than a substring of a message.
        let OrderError::InvalidQty { qty } = error.downcast_ref::<OrderError>()? else {
            return None;
        };
        Some(toni::GrpcStatus::new(
            toni::GrpcCode::FailedPrecondition,
            format!("qty must be positive, got {qty}"),
        ))
    }
}

// ─── Service ────────────────────────────────────────────────────────────────
//
// `#[controller]` declares the service as a dispatch target; the
// `#[inject]`-annotated fields are resolved from the module's providers
// list. `#[grpc_methods]` writes the proto-trait impl around these
// handlers, with the enhancer pipeline the `#[use_*]` attributes declare — at bind time the
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

#[grpc_methods(orders_pb::orders_server::Orders)]
#[use_guards(AuthGuard)]
#[use_error_handlers(QtyErrorHandler)]
impl OrdersGrpcService {
    #[grpc_method]
    #[use_interceptors(LoggingInterceptor)]
    async fn create(
        &self,
        toni::extractors::Payload(req): toni::extractors::Payload<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::CreateOrderResponse, OrderError> {
        if req.qty == 0 {
            // The chain claims this one and answers FailedPrecondition. With
            // the handler removed, the kind decides: INVALID_ARGUMENT.
            return Err(OrderError::InvalidQty { qty: req.qty });
        }
        if req.item == "unobtainium" {
            // Nothing claims this one, so the kind is the answer: ABORTED.
            return Err(OrderError::OutOfStock { item: req.item });
        }
        let id = self.counter.next_id();
        Ok(orders_pb::CreateOrderResponse {
            id,
            item: req.item.clone(),
            qty: req.qty,
            status: format!("created:{}", req.item),
        })
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
