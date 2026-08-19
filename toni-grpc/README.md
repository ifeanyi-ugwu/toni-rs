# toni-grpc

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

gRPC transport adapter for the [Toni](https://github.com/monterxto/toni-rs) framework.

Drives a [`tonic`](https://github.com/hyperium/tonic) server through Toni's bind / serve / drain lifecycle, with first-class dependency injection, per-call guards, interceptors, error handlers, and panic recovery on every method dispatched through `#[grpc_methods]`.

## Features

- ✅ **`#[grpc_service]` + `#[grpc_methods]`** — register a tonic-generated service trait impl as a DI provider and have the framework wrap it for you
- ✅ **Dependency Injection** — `#[inject]` fields resolved from the module's providers at construction
- ✅ **Guards** (`#[use_guards]`) — per-call boolean check, rejection surfaces as `PermissionDenied`
- ✅ **Interceptors** (`#[use_interceptors]`) — around-handler chain, can short-circuit with a typed `GrpcStatus`
- ✅ **Error handlers** (`#[use_error_handlers]`) — claim and remap user-returned `Err(Status)` or caught panics; pass-through unchanged when no handler claims
- ✅ **Panic recovery** — a panicking handler surfaces as `Internal` rather than tearing down the connection
- ✅ **All four call modes** — unary, server-streaming, client-streaming, bidirectional streaming
- ✅ **Graceful shutdown** with configurable drain timeout
- ✅ **Per-request `rpc.request` tracing span** carrying service / method / peer

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
toni = "0.2"
toni-grpc = "0.1"
tonic = "0.14"
tokio = { version = "1", features = ["full"] }

[build-dependencies]
tonic-prost-build = "0.14"
```

A `build.rs` compiles your `.proto` into Rust:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/orders.proto")?;
    Ok(())
}
```

## Quick Start

`proto/orders.proto`:

```protobuf
syntax = "proto3";
package toni_examples.orders;

service Orders {
    rpc Create (CreateOrderRequest) returns (CreateOrderResponse);
}

message CreateOrderRequest { string item = 1; uint32 qty = 2; }
message CreateOrderResponse { uint64 id = 1; string status = 2; }
```

`src/main.rs`:

```rust
use std::net::SocketAddr;
use toni::ToniFactory;
use toni_macros::{grpc_methods, grpc_service, injectable, module};

mod orders_pb { tonic::include_proto!("toni_examples.orders"); }
use orders_pb::orders_server::{Orders, OrdersServer};

#[injectable]
pub struct OrdersCounter {}

#[grpc_service(pub struct OrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl OrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self { Self { counter } }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for OrdersGrpcService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 1,
            status: format!("created:{}", req.item),
        }))
    }
}

#[module(controllers: [OrdersGrpcService], providers: [OrdersCounter])]
struct AppModule;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
        let mut app = ToniFactory::create(AppModule).await;
        app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(addr)).unwrap();
        app.start().await.unwrap();
    }).await;
}
```

Run it and hit it with [`grpcurl`](https://github.com/fullstorydev/grpcurl):

```bash
grpcurl -plaintext \
    -d '{"item":"keyboard","qty":3}' \
    -proto proto/orders.proto -import-path proto \
    127.0.0.1:50051 toni_examples.orders.Orders/Create
```

## Enhancers

Guards, interceptors, and error handlers are declared as DI providers and applied to a `#[grpc_methods]` impl via `#[use_*]` attributes — at the service level (every method) or per method.

```rust
#[grpc_methods]
#[tonic::async_trait]
#[use_guards(AuthGuard)]
#[use_error_handlers(QtyErrorHandler)]
impl Orders for OrdersGrpcService {
    #[use_interceptors(LoggingInterceptor)]
    async fn create(&self, req: tonic::Request<...>) -> Result<..., tonic::Status> {
        // your handler
    }
}
```

### Guards

A guard is a provider that implements `Guard<GrpcContext>`. The framework runs the resolved chain before the handler; a `false` return short-circuits the call with `PermissionDenied`.

```rust
#[injectable]
pub struct AuthGuard {}

#[toni::async_trait]
impl toni::traits_helpers::Guard<toni::GrpcContext> for AuthGuard {
    async fn can_activate(&self, ctx: &toni::GrpcContext) -> bool {
        ctx.get_metadata("authorization") == Some("Bearer secret-token")
    }
}
```

### Interceptors

An interceptor is a provider that implements `Interceptor<GrpcContext>`. The chain wraps the user delegation — `next.run(ctx).await` proceeds; calling `ctx.set_response(Err(GrpcStatus::...))` and skipping `next.run` short-circuits.

```rust
#[injectable]
pub struct LoggingInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for LoggingInterceptor {
    async fn intercept(
        &self,
        ctx: &mut toni::GrpcContext,
        next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        tracing::info!(method = %ctx.method(), "before");
        next.run(ctx).await;
        tracing::info!(method = %ctx.method(), "after");
    }
}
```

### Error handlers

An error handler is a provider that implements `ErrorHandler<GrpcContext, GrpcStatus>`. The chain offers it every user-returned `Err(Status)` (wrapped as `GrpcStatus`) and every caught handler panic (as a typed `PanicRecovered`). Returning `Some(GrpcStatus)` claims the response; `None` lets the next handler in the chain decide, falling back to the original on full miss.

```rust
#[injectable]
pub struct QtyErrorHandler {}

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
```

### Pipes

Pipes (`#[use_pipes]`) are **not** supported on `#[grpc_methods]`. The proto payload is method-typed and can't sit in a non-generic `GrpcContext`, and a metadata-only pipe role adds no expressive power over interceptors. Use an interceptor if you need to mutate `ctx.metadata_mut()` before the handler runs.

## Streaming

All four call modes work through `#[grpc_methods]` without per-mode setup. Declare the associated streams the tonic-generated trait expects and yield from your handler as usual:

```rust
type WatchProgressStream = Pin<Box<dyn Stream<Item = Result<ProgressEvent, Status>> + Send>>;

async fn watch_progress(
    &self,
    request: tonic::Request<WatchRequest>,
) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
    let stream = futures_util::stream::iter([/* ... */]);
    Ok(tonic::Response::new(Box::pin(stream)))
}
```

Enhancers apply to streaming methods the same way they apply to unary ones. Guards run before the response stream is opened; interceptors wrap the call up to the point the stream is returned; error handlers fire if the handler returns `Err` or panics. Mid-stream errors emitted *inside* the stream itself are produced by your code and pass through unchanged.

## Lifecycle

### Graceful shutdown

`close()` signals tonic's `serve_with_incoming_shutdown` and starts a drain timer. The default budget is 10 seconds; configure with `with_drain_timeout`:

```rust
let adapter = toni_grpc::GrpcAdapter::new(addr)
    .with_drain_timeout(Duration::from_secs(30));

// Wait forever — no abort:
let adapter = adapter.with_drain_timeout(None);
```

When the timer elapses with in-flight calls still running, the serve future is dropped — connections close, streaming clients see `UNAVAILABLE`, and the framework moves on to the next shutdown step.

### Tracing

Every dispatched method is wrapped in a `tracing::info_span!("rpc.request", transport="grpc", pattern="…", id=…, peer=…)` span. Any `tracing` event emitted from the user handler (or from the adapter itself) automatically inherits those fields so an operator scanning logs can correlate every line back to the originating call. The `rpc_tracing` example in the repo's `examples/` directory shows this in action across TCP / UDP / gRPC simultaneously.

## Registering services without DI

`GrpcAdapter::add_service` accepts any tonic-generated service handle, so you can mix DI-registered services with manually-built ones:

```rust
let adapter = toni_grpc::GrpcAdapter::new(addr)
    .add_service(SomeOtherServer::new(other_handler));
app.use_grpc_adapter(adapter)?;
```

Manually-registered services don't get enhancer support — they're a passthrough to tonic.

## Example

A complete runnable example wiring DI + guards + interceptors + error handlers lives at [`examples/grpc_service.rs`](../examples/grpc_service.rs):

```bash
cargo run --example grpc_service
```

The example's header comment includes copy-paste `grpcurl` commands for the authorised, unauthorised, and error-remap paths.

## License

MIT
