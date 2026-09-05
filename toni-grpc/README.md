# toni-grpc

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

gRPC transport adapter for the [Toni](https://github.com/monterxto/toni-rs) framework.

Drives a [`tonic`](https://github.com/hyperium/tonic) server through Toni's bind / serve / drain lifecycle, with first-class dependency injection, per-call guards, interceptors, error handlers, and panic recovery on every method dispatched through `#[grpc_methods]`.

## Features

- ✅ **`#[controller]` + `#[grpc_methods]`** — write handlers in toni's shapes and have the framework write the tonic trait impl and register it
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
use toni::extractors::Payload;
use toni_macros::{controller, grpc_methods, injectable, module, new};

mod orders_pb { tonic::include_proto!("toni_examples.orders"); }

#[injectable]
pub struct OrdersCounter {}

#[controller]
pub struct OrdersGrpcService {
    #[inject] counter: OrdersCounter,
}

#[grpc_methods(orders_pb::orders_server::Orders)]
impl OrdersGrpcService {
    #[new]
    pub fn new(counter: OrdersCounter) -> Self { Self { counter } }

    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::CreateOrderResponse, OrderError> {
        Ok(orders_pb::CreateOrderResponse {
            id: 1,
            status: format!("created:{}", req.item),
        })
    }
}

#[module(controllers: [OrdersGrpcService], providers: [OrdersCounter])]
struct AppModule;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
        let mut app = ToniFactory::create(AppModule).await.unwrap();
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
#[grpc_methods(orders_pb::orders_server::Orders)]
#[use_guards(AuthGuard)]
#[use_error_handlers(QtyErrorHandler)]
impl OrdersGrpcService {
    #[grpc_method]
    #[use_interceptors(LoggingInterceptor)]
    async fn create(&self, Payload(req): Payload<CreateOrderRequest>)
        -> Result<CreateOrderResponse, OrderError>
    {
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
        ctx.header("authorization") == Some("Bearer secret-token")
    }
}
```

### Interceptors

An interceptor is a provider that implements `Interceptor<GrpcContext, GrpcHandlerResult>`. The chain wraps the handler — `next.run(ctx).await` proceeds; returning without calling it short-circuits.

```rust
#[injectable]
pub struct LoggingInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext, toni::GrpcHandlerResult>
    for LoggingInterceptor {
    async fn intercept(
        &self,
        ctx: &toni::GrpcContext,
        next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext, toni::GrpcHandlerResult>>,
    ) -> toni::GrpcHandlerResult {
        tracing::info!(method = %ctx.method(), "before");
        let answer = next.run(ctx).await;
        tracing::info!(method = %ctx.method(), "after");
        answer
    }
}
```

### Error handlers

An error handler is a provider that implements `ErrorHandler<GrpcContext, GrpcStatus>`. The chain offers it every error a handler returned and every caught panic (as a typed `PanicRecovered`). Returning `Some(GrpcStatus)` claims the answer; `None` lets the next handler decide, falling back on full miss to the status the handler already answered with.

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
        let OrderError::InvalidQty { qty } = error.downcast_ref::<OrderError>()? else {
            return None;
        };
        Some(toni::GrpcStatus::new(
            toni::GrpcCode::FailedPrecondition,
            format!("qty must be positive, got {qty}"),
        ))
    }
}
```

### Naming a code

`grpc_code` maps the eleven `ErrorKind`s onto the canonical codes. For one outside that table — `FailedPrecondition`, `OutOfRange`, `AlreadyExists` — a handler returns a `GrpcStatus`, which is itself a `toni::Error`:

```rust
#[grpc_method]
async fn reserve(&self, Payload(req): Payload<ReserveRequest>)
    -> Result<ReserveReply, GrpcStatus>
{
    if !self.window_open() {
        return Err(GrpcStatus::new(GrpcCode::FailedPrecondition, "the booking window is closed"));
    }
    ...
}
```

`caused_by` keeps a domain error on a status whose code was named, so the code goes to the wire and a `#[catch(WindowClosed)]` handler still matches the type:

```rust
Err(GrpcStatus::new(GrpcCode::OutOfRange, "past the last slot").caused_by(WindowClosed))
```

## Streaming

All four call modes work through `#[grpc_methods]`. Which one a method serves is read from its own signature: `Inbound<T>` for a request the caller streams, `#[grpc_stream]` for a reply the handler streams, both together for bidirectional. The associated stream type the tonic-generated trait declares is written for you.

```rust
#[grpc_stream]
async fn watch_progress(
    &self,
    Payload(req): Payload<WatchRequest>,
) -> Result<impl Stream<Item = Result<ProgressEvent, OrderError>> + Send + 'static, OrderError> {
    Ok(futures_util::stream::iter([/* ... */]))
}

#[grpc_method]
async fn bulk_create(&self, mut inbound: Inbound<CreateOrderRequest>)
    -> Result<BulkCreateResponse, OrderError>
{
    while let Some(req) = inbound.next().await { /* ... */ }
    Ok(BulkCreateResponse { /* ... */ })
}
```

`#[grpc_stream]` reads the associated type's name off the method — `watch_progress` declares `WatchProgressStream` — which is the pairing tonic-build derives from one proto identifier. A trait that names them independently says so: `#[grpc_stream(StreamProgressStream)]`.

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
