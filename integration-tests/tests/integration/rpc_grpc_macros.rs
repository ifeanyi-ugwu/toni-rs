//! End-to-end coverage for the `#[grpc_service]` + `#[grpc_methods]` macros:
//!
//! - `#[grpc_service]` on a struct + its inherent impl makes it an injectable
//!   DI provider that the framework discovers as a gRPC service.
//! - `#[grpc_methods]` on the proto-trait impl emits a `GrpcServiceTrait`
//!   that wraps `self` in the inferred `*Server` and registers it with
//!   the framework's gRPC adapter at bind time.
//! - The user never types `*Server::new(handler)` and never calls
//!   `adapter.add_service()` — DI + module registration is the entire
//!   wiring story.
//! - All four call modes (unary, server-streaming, client-streaming, bidi)
//!   work through the macros without per-mode special handling — the macro
//!   just hands tonic an instance of the trait impl, and tonic dispatches.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use toni::ToniFactory;
use toni::enhancer::error_handler;
use toni::{guard, interceptor};
use toni_macros::{
    grpc_methods, grpc_service, injectable, module, use_error_handlers, use_guards,
    use_interceptors,
};

mod orders_pb {
    tonic::include_proto!("toni_test.orders");
}

use orders_pb::orders_client::OrdersClient;
use orders_pb::orders_server::{Orders, OrdersServer};

#[injectable(pub struct OrdersCounter {
    seq: Arc<AtomicU64>,
})]
impl OrdersCounter {
    pub fn new() -> Self {
        Self {
            seq: Arc::new(AtomicU64::new(1000)),
        }
    }

    fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }
}

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
impl Orders for OrdersGrpcService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        if req.qty == 0 {
            return Err(tonic::Status::invalid_argument("qty must be positive"));
        }
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let id = request.into_inner().id;
        let stream = futures_util::stream::iter(
            ["queued", "picked", "shipped"]
                .into_iter()
                .map(move |status| {
                    Ok(orders_pb::ProgressEvent {
                        id,
                        status: status.to_string(),
                    })
                }),
        );
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        let mut stream = request.into_inner();
        let mut created: u32 = 0;
        let first_id = self.counter.next_id();
        // Reserve subsequent ids contiguously so the response can summarise.
        while let Some(item) = stream.next().await {
            let _req = item?;
            if created > 0 {
                self.counter.next_id();
            }
            created += 1;
        }
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created,
            first_id,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let mut inbound = request.into_inner();
        let counter = self.counter.clone();
        let outbound = async_stream::stream! {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(m) => yield Ok(orders_pb::ChatMessage {
                        text: m.text,
                        id: counter.next_id(),
                    }),
                    Err(e) => yield Err(e),
                }
            }
        };
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, OrdersGrpcService])]
struct GrpcMacrosModule;

// ── enhancer-coverage fixtures ──────────────────────────────────────────────
//
// A second service alongside `OrdersGrpcService` that exercises
// `#[use_guards]` at both service- and method-level. Each guard test boots
// `GuardedGrpcModule` (this module) on its own port, so the duplicate
// `impl Orders for ...` blocks never coexist on a running server.

#[injectable(pub struct AuthGuard {})]
#[guard(grpc)]
impl AuthGuard {}

#[toni::async_trait]
impl toni::traits_helpers::Guard<toni::GrpcContext> for AuthGuard {
    async fn can_activate(&self, ctx: &toni::GrpcContext) -> bool {
        ctx.get_metadata("authorization").is_some()
    }
}

#[injectable(pub struct AdminGuard {})]
#[guard(grpc)]
impl AdminGuard {}

#[toni::async_trait]
impl toni::traits_helpers::Guard<toni::GrpcContext> for AdminGuard {
    async fn can_activate(&self, ctx: &toni::GrpcContext) -> bool {
        ctx.get_metadata("x-role") == Some("admin")
    }
}

#[grpc_service(pub struct GuardedOrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl GuardedOrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self {
        Self { counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_guards(AuthGuard)]
impl Orders for GuardedOrdersGrpcService {
    #[use_guards(AdminGuard)]
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let id = request.into_inner().id;
        let stream = futures_util::stream::iter(["queued"].into_iter().map(move |status| {
            Ok(orders_pb::ProgressEvent {
                id,
                status: status.to_string(),
            })
        }));
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let _ = request.into_inner();
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, AuthGuard, AdminGuard, GuardedOrdersGrpcService])]
struct GuardedGrpcModule;

async fn boot_guarded() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(GuardedGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

// ── interceptor-coverage fixtures ───────────────────────────────────────────
//
// Process-global event log so a test can assert on the order interceptors
// fired around the user delegation. Each test calls `drain_interceptor_log()`
// at the start to isolate from neighbours run earlier in the same process.

static INTERCEPTOR_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Held for the duration of each interceptor test so the three of them
/// don't race on the shared `INTERCEPTOR_LOG`. cargo runs integration
/// tests in parallel by default.
static INTERCEPTOR_TEST_SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn log_interceptor(msg: &str) {
    INTERCEPTOR_LOG.lock().unwrap().push(msg.to_string());
}

fn drain_interceptor_log() -> Vec<String> {
    let mut g = INTERCEPTOR_LOG.lock().unwrap();
    let v = g.clone();
    g.clear();
    v
}

fn lock_interceptor_test() -> std::sync::MutexGuard<'static, ()> {
    INTERCEPTOR_TEST_SERIALIZE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[injectable(pub struct ServiceInterceptor {})]
#[interceptor(grpc)]
impl ServiceInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for ServiceInterceptor {
    async fn intercept(
        &self,
        ctx: &mut toni::GrpcContext,
        next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        log_interceptor("service:before");
        next.run(ctx).await;
        log_interceptor("service:after");
    }
}

#[injectable(pub struct MethodInterceptor {})]
#[interceptor(grpc)]
impl MethodInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for MethodInterceptor {
    async fn intercept(
        &self,
        ctx: &mut toni::GrpcContext,
        next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        log_interceptor("method:before");
        next.run(ctx).await;
        log_interceptor("method:after");
    }
}

#[injectable(pub struct DenyInterceptor {})]
#[interceptor(grpc)]
impl DenyInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for DenyInterceptor {
    async fn intercept(
        &self,
        ctx: &mut toni::GrpcContext,
        _next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        log_interceptor("deny:short-circuit");
        ctx.set_response(Err(toni::GrpcStatus::permission_denied(
            "blocked by interceptor",
        )));
        // Deliberately skip `_next.run(ctx).await` to short-circuit.
    }
}

#[grpc_service(pub struct InterceptedOrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl InterceptedOrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self {
        Self { counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_interceptors(ServiceInterceptor)]
impl Orders for InterceptedOrdersGrpcService {
    #[use_interceptors(MethodInterceptor)]
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        log_interceptor("handler:run");
        let req = request.into_inner();
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        log_interceptor("handler:run");
        let id = request.into_inner().id;
        let stream = futures_util::stream::iter(["queued"].into_iter().map(move |status| {
            Ok(orders_pb::ProgressEvent {
                id,
                status: status.to_string(),
            })
        }));
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let _ = request.into_inner();
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [
    OrdersCounter,
    ServiceInterceptor,
    MethodInterceptor,
    InterceptedOrdersGrpcService,
])]
struct InterceptedGrpcModule;

/// Same shape as the deny module below but with `MethodInterceptor` swapped
/// for `DenyInterceptor` on the `create` method, so the short-circuit test
/// has an isolated server.
#[grpc_service(pub struct DenyOrdersGrpcService {
    #[inject] counter: OrdersCounter,
})]
impl DenyOrdersGrpcService {
    pub fn new(counter: OrdersCounter) -> Self {
        Self { counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_interceptors(DenyInterceptor)]
impl Orders for DenyOrdersGrpcService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        log_interceptor("handler:run");
        let req = request.into_inner();
        let id = self.counter.next_id();
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id,
            status: format!("created:{}", req.item),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let stream = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, DenyInterceptor, DenyOrdersGrpcService])]
struct DenyGrpcModule;

async fn boot_intercepted() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(InterceptedGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn boot_deny() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(DenyGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

/// Boots the gRPC server with the default drain timeout.
async fn boot() -> (u16, toni::ShutdownHandle) {
    boot_with(|a| a).await
}

/// Boots the gRPC server, applying a custom configuration to the adapter
/// before it's registered (e.g. `with_drain_timeout`).
async fn boot_with<F>(configure: F) -> (u16, toni::ShutdownHandle)
where
    F: FnOnce(toni_grpc::GrpcAdapter) -> toni_grpc::GrpcAdapter + Send + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = configure(toni_grpc::GrpcAdapter::new(addr));
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(GrpcMacrosModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound
            .grpc
            .expect("BoundAdapters.grpc must be populated")
            .port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    let endpoint = format!("http://127.0.0.1:{}", port);
    tonic::transport::Endpoint::from_shared(endpoint)
        .unwrap()
        .connect()
        .await
        .map(OrdersClient::new)
        .expect("gRPC connect should succeed")
}

#[tokio_localset_test::localset_test]
async fn grpc_service_macro_di_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let resp = tokio::time::timeout(
        Duration::from_secs(2),
        client.create(orders_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 3,
        }),
    )
    .await
    .expect("call must reply within 2s")
    .expect("call must succeed")
    .into_inner();

    assert!(resp.id >= 1000, "id should come from the injected counter, got {}", resp.id);
    assert_eq!(resp.status, "created:keyboard");

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".to_string(),
            qty: 0,
        })
        .await
        .expect_err("qty=0 must fail");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

#[tokio_localset_test::localset_test]
async fn grpc_server_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut stream = client
        .watch_progress(orders_pb::WatchRequest { id: 42 })
        .await
        .expect("server-streaming call must succeed")
        .into_inner();

    let mut statuses = Vec::new();
    while let Some(item) = stream.next().await {
        let evt = item.expect("stream item must be Ok");
        assert_eq!(evt.id, 42, "server-streaming events must echo the request id");
        statuses.push(evt.status);
    }
    assert_eq!(statuses, vec!["queued", "picked", "shipped"]);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

#[tokio_localset_test::localset_test]
async fn grpc_client_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let outbound = futures_util::stream::iter(vec![
        orders_pb::CreateOrderRequest { item: "a".into(), qty: 1 },
        orders_pb::CreateOrderRequest { item: "b".into(), qty: 2 },
        orders_pb::CreateOrderRequest { item: "c".into(), qty: 3 },
    ]);

    let resp = client
        .bulk_create(outbound)
        .await
        .expect("client-streaming call must succeed")
        .into_inner();

    assert_eq!(resp.created, 3);
    assert!(
        resp.first_id >= 1000,
        "first_id should come from the injected counter, got {}",
        resp.first_id
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// Without drain enforcement a never-closing bidi stream pins the server
/// open after `shutdown()` because tonic's `serve_with_incoming_shutdown`
/// waits for in-flight handlers to return. With `with_drain_timeout` the
/// budget elapses, the serve future is dropped, and the in-flight stream
/// is aborted (clients see UNAVAILABLE).
#[tokio_localset_test::localset_test]
async fn grpc_drain_timeout_aborts_long_running_streams() {
    let drain = Duration::from_millis(150);
    let (port, shutdown) = boot_with(move |a| a.with_drain_timeout(drain)).await;
    let mut client = connect(port).await;

    // Open a bidi stream where the client never closes its outbound channel
    // — the server-side `chat()` handler stays parked in `inbound.next().await`.
    let (tx, rx) = tokio::sync::mpsc::channel::<orders_pb::ChatMessage>(1);
    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut stream = client
        .chat(outbound)
        .await
        .expect("bidi call must succeed")
        .into_inner();

    // Send one message so the server enters the handler and starts blocking.
    tx.send(orders_pb::ChatMessage {
        text: "ping".into(),
        id: 0,
    })
    .await
    .unwrap();
    // Wait for the server's reply so we know the handler is mid-flight.
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("first echo should arrive")
        .expect("stream item present")
        .expect("item is Ok");

    // Trigger shutdown. Without enforcement, completed() would hang because
    // the bidi stream is still in-flight from the server's perspective.
    let before = tokio::time::Instant::now();
    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(1), shutdown.completed())
        .await
        .expect("drain budget should bound shutdown");
    let elapsed = before.elapsed();
    assert!(
        elapsed >= drain,
        "shutdown raced past the drain budget — was the timer skipped? elapsed={:?}",
        elapsed,
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "shutdown took noticeably longer than the drain budget — was it enforced? elapsed={:?}",
        elapsed,
    );
}

#[tokio_localset_test::localset_test]
async fn grpc_bidi_streaming_round_trip() {
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let outbound = futures_util::stream::iter(vec![
        orders_pb::ChatMessage { text: "hello".into(), id: 0 },
        orders_pb::ChatMessage { text: "world".into(), id: 0 },
    ]);

    let mut inbound = client
        .chat(outbound)
        .await
        .expect("bidi call must succeed")
        .into_inner();

    let mut texts = Vec::new();
    let mut ids = Vec::new();
    while let Some(item) = inbound.next().await {
        let m = item.expect("bidi item must be Ok");
        texts.push(m.text);
        ids.push(m.id);
    }
    assert_eq!(texts, vec!["hello", "world"]);
    assert_eq!(ids.len(), 2);
    assert!(ids[0] >= 1000 && ids[1] == ids[0] + 1, "ids must come from the counter");

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

// ── enhancer tests ──────────────────────────────────────────────────────────

/// Service-level `#[use_guards(AuthGuard)]` lets through a request that
/// carries the `authorization` metadata the guard checks for.
#[tokio_localset_test::localset_test]
async fn grpc_guard_accepts_request() {
    let (port, shutdown) = boot_guarded().await;
    let mut client = connect(port).await;

    let mut req = tonic::Request::new(orders_pb::WatchRequest { id: 7 });
    req.metadata_mut()
        .insert("authorization", "Bearer abc".parse().unwrap());

    let mut stream = client
        .watch_progress(req)
        .await
        .expect("guard with valid metadata must accept")
        .into_inner();
    let first = stream
        .next()
        .await
        .expect("stream item present")
        .expect("stream item ok");
    assert_eq!(first.id, 7);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// A missing `authorization` header makes `AuthGuard` reject — the wire
/// status is `PERMISSION_DENIED`, mirroring how guard rejections surface
/// across the framework's other transports.
#[tokio_localset_test::localset_test]
async fn grpc_guard_rejects_with_permission_denied() {
    let (port, shutdown) = boot_guarded().await;
    let mut client = connect(port).await;

    let err = client
        .watch_progress(orders_pb::WatchRequest { id: 1 })
        .await
        .expect_err("missing metadata must be rejected");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// Method-level `#[use_guards]` stacks on top of service-level: `create`
/// runs both `AuthGuard` *and* `AdminGuard`. The service-level guard alone
/// (just `authorization`) is no longer enough; the request must also carry
/// the admin role for `create` to dispatch.
#[tokio_localset_test::localset_test]
async fn grpc_guard_method_level_stacks_on_block_level() {
    let (port, shutdown) = boot_guarded().await;
    let mut client = connect(port).await;

    // Auth header alone passes `watch_progress` but not `create`.
    let mut auth_only = tonic::Request::new(orders_pb::CreateOrderRequest {
        item: "shoes".into(),
        qty: 1,
    });
    auth_only
        .metadata_mut()
        .insert("authorization", "Bearer abc".parse().unwrap());
    let err = client
        .create(auth_only)
        .await
        .expect_err("method-level AdminGuard must reject when x-role is missing");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    // Both headers present → both guards accept → call dispatches.
    let mut both = tonic::Request::new(orders_pb::CreateOrderRequest {
        item: "shoes".into(),
        qty: 1,
    });
    both.metadata_mut()
        .insert("authorization", "Bearer abc".parse().unwrap());
    both.metadata_mut().insert("x-role", "admin".parse().unwrap());
    let resp = client
        .create(both)
        .await
        .expect("both guards must accept when both headers are set")
        .into_inner();
    assert_eq!(resp.status, "created:shoes");

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

// ── error-handler fixtures ─────────────────────────────────────────────────

/// Claims errors whose `to_string()` contains `"remap-me"`; everything else
/// passes through unchanged. Lets one server cover both the claim path and
/// the pass-through path in adjacent tests.
#[injectable(pub struct ConditionalErrorHandler {})]
#[error_handler(grpc)]
impl ConditionalErrorHandler {}

#[toni::async_trait]
impl toni::traits_helpers::ErrorHandler<toni::GrpcContext, toni::GrpcStatus>
    for ConditionalErrorHandler
{
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &toni::GrpcContext,
    ) -> ::std::option::Option<toni::GrpcStatus> {
        let msg = error.to_string();
        if msg.contains("remap-me") {
            Some(toni::GrpcStatus::new(
                toni::GrpcCode::FailedPrecondition,
                "remapped by handler",
            ))
        } else {
            None
        }
    }
}

#[grpc_service(pub struct ErrorHandledOrdersGrpcService {
    #[inject] _counter: OrdersCounter,
})]
impl ErrorHandledOrdersGrpcService {
    pub fn new(_counter: OrdersCounter) -> Self {
        Self { _counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_error_handlers(ConditionalErrorHandler)]
impl Orders for ErrorHandledOrdersGrpcService {
    async fn create(
        &self,
        request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        // `item` is echoed into the Err message so the test can steer the
        // handler's `to_string()` match without crafting a custom error type.
        Err(tonic::Status::invalid_argument(format!(
            "user-said: {}",
            req.item
        )))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let stream = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [
    OrdersCounter,
    ConditionalErrorHandler,
    ErrorHandledOrdersGrpcService,
])]
struct ErrorHandledGrpcModule;

async fn boot_error_handled() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ErrorHandledGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

/// `create` panics. No error handler registered, so the framework's
/// default panic recovery is what produces the wire reply.
#[grpc_service(pub struct PanickyOrdersGrpcService {
    #[inject] _counter: OrdersCounter,
})]
impl PanickyOrdersGrpcService {
    pub fn new(_counter: OrdersCounter) -> Self {
        Self { _counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
impl Orders for PanickyOrdersGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        panic!("boom from handler")
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let stream = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, PanickyOrdersGrpcService])]
struct PanickyGrpcModule;

async fn boot_panicky() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(PanickyGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

// ── interceptor tests ───────────────────────────────────────────────────────

/// A service-level interceptor wraps the user delegation: `before` runs,
/// the handler runs in the middle, `after` runs as the chain unwinds.
#[tokio_localset_test::localset_test]
async fn grpc_interceptor_runs_around_handler() {
    let _serial = lock_interceptor_test();
    drain_interceptor_log();
    let (port, shutdown) = boot_intercepted().await;
    let mut client = connect(port).await;

    let resp = client
        .watch_progress(orders_pb::WatchRequest { id: 1 })
        .await
        .expect("call must succeed")
        .into_inner();
    drop(resp);

    let log = drain_interceptor_log();
    assert_eq!(
        log,
        vec!["service:before", "handler:run", "service:after"],
        "interceptor must wrap the user delegation in before/after order",
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// An interceptor that calls `ctx.set_response(Err(...))` and skips
/// `next.run` short-circuits the call. The user handler never runs and
/// the wire status comes from the interceptor's `GrpcStatus`.
#[tokio_localset_test::localset_test]
async fn grpc_interceptor_short_circuits_with_error() {
    let _serial = lock_interceptor_test();
    drain_interceptor_log();
    let (port, shutdown) = boot_deny().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".into(),
            qty: 1,
        })
        .await
        .expect_err("interceptor must short-circuit");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    let log = drain_interceptor_log();
    assert_eq!(log, vec!["deny:short-circuit"]);
    assert!(
        !log.iter().any(|m| m == "handler:run"),
        "handler must not run when interceptor short-circuits",
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// Method-level interceptors stack inside service-level ones: the
/// service-level `before` runs first, then the method-level `before`,
/// the handler runs, then unwinds in reverse (method-level `after`,
/// service-level `after`).
#[tokio_localset_test::localset_test]
async fn grpc_interceptor_method_level_stacks_inside_service_level() {
    let _serial = lock_interceptor_test();
    drain_interceptor_log();
    let (port, shutdown) = boot_intercepted().await;
    let mut client = connect(port).await;

    let resp = client
        .create(orders_pb::CreateOrderRequest {
            item: "shoes".into(),
            qty: 1,
        })
        .await
        .expect("call must succeed")
        .into_inner();
    assert_eq!(resp.status, "created:shoes");

    let log = drain_interceptor_log();
    assert_eq!(
        log,
        vec![
            "service:before",
            "method:before",
            "handler:run",
            "method:after",
            "service:after",
        ],
        "method-level interceptor must nest inside the service-level one",
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

// ── error-handler tests ────────────────────────────────────────────────────

/// A registered error handler whose `handle_error` returns `Some` claims
/// the response: the wire status comes from the handler's `GrpcStatus`,
/// not the user's original `Err(Status)`.
#[tokio_localset_test::localset_test]
async fn grpc_error_handler_claims_and_remaps_user_err() {
    let (port, shutdown) = boot_error_handled().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            // The handler matches on this substring and remaps.
            item: "remap-me".into(),
            qty: 0,
        })
        .await
        .expect_err("user method returns Err; handler must claim it");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "remapped by handler");

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// When the registered handler returns `None`, the user's original
/// `Err(Status)` passes through unchanged. Same server as the claim test
/// — the only difference is the request payload, which the handler uses
/// to decide whether to claim.
#[tokio_localset_test::localset_test]
async fn grpc_error_handler_passes_through_when_no_claim() {
    let (port, shutdown) = boot_error_handled().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "leave-alone".into(),
            qty: 0,
        })
        .await
        .expect_err("user method returns Err");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(
        err.message().contains("leave-alone"),
        "pass-through must preserve the user's original message; got {:?}",
        err.message()
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// A panicking handler must not tear down the process or the connection
/// — the framework catches the panic and surfaces it as `Internal` to
/// the wire. The panic payload bubbles into the status message so an
/// operator inspecting the response can correlate.
#[tokio_localset_test::localset_test]
async fn grpc_panic_in_handler_surfaces_as_internal() {
    let (port, shutdown) = boot_panicky().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".into(),
            qty: 1,
        })
        .await
        .expect_err("panicking handler must produce an Err — not a connection drop");

    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(
        err.message().contains("boom from handler"),
        "panic payload must propagate into the status message; got {:?}",
        err.message()
    );

    // A second call on the same channel proves the server stayed up
    // through the panic — the catch_unwind wraps each handler invocation,
    // not the whole server.
    let err2 = client
        .create(orders_pb::CreateOrderRequest {
            item: "again".into(),
            qty: 1,
        })
        .await
        .expect_err("subsequent panicking call must also surface as Err");
    assert_eq!(err2.code(), tonic::Code::Internal);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

// ── pipeline-segment panic coverage (guard + interceptor) ──────────────────

#[injectable(pub struct PanickingGrpcGuard {})]
#[guard(grpc)]
impl PanickingGrpcGuard {}

#[toni::async_trait]
impl toni::traits_helpers::Guard<toni::GrpcContext> for PanickingGrpcGuard {
    async fn can_activate(&self, _ctx: &toni::GrpcContext) -> bool {
        panic!("guard kaboom");
    }
}

#[grpc_service(pub struct GuardPanicGrpcService {
    #[inject] _counter: OrdersCounter,
})]
impl GuardPanicGrpcService {
    pub fn new(_counter: OrdersCounter) -> Self {
        Self { _counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_guards(PanickingGrpcGuard)]
impl Orders for GuardPanicGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 0,
            status: "unreachable".into(),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let stream = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, PanickingGrpcGuard, GuardPanicGrpcService])]
struct GuardPanicGrpcModule;

#[injectable(pub struct PanickingGrpcInterceptor {})]
#[interceptor(grpc)]
impl PanickingGrpcInterceptor {}

#[toni::async_trait]
impl toni::traits_helpers::Interceptor<toni::GrpcContext> for PanickingGrpcInterceptor {
    async fn intercept(
        &self,
        _ctx: &mut toni::GrpcContext,
        _next: Box<dyn toni::traits_helpers::InterceptorNext<toni::GrpcContext>>,
    ) {
        panic!("interceptor kaboom");
    }
}

#[grpc_service(pub struct InterceptorPanicGrpcService {
    #[inject] _counter: OrdersCounter,
})]
impl InterceptorPanicGrpcService {
    pub fn new(_counter: OrdersCounter) -> Self {
        Self { _counter }
    }
}

#[grpc_methods]
#[tonic::async_trait]
#[use_interceptors(PanickingGrpcInterceptor)]
impl Orders for InterceptorPanicGrpcService {
    async fn create(
        &self,
        _request: tonic::Request<orders_pb::CreateOrderRequest>,
    ) -> Result<tonic::Response<orders_pb::CreateOrderResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::CreateOrderResponse {
            id: 0,
            status: "unreachable".into(),
        }))
    }

    type WatchProgressStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ProgressEvent, tonic::Status>> + Send>>;

    async fn watch_progress(
        &self,
        _request: tonic::Request<orders_pb::WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchProgressStream>, tonic::Status> {
        let stream = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn bulk_create(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::CreateOrderRequest>>,
    ) -> Result<tonic::Response<orders_pb::BulkCreateResponse>, tonic::Status> {
        Ok(tonic::Response::new(orders_pb::BulkCreateResponse {
            created: 0,
            first_id: 0,
        }))
    }

    type ChatStream =
        Pin<Box<dyn Stream<Item = Result<orders_pb::ChatMessage, tonic::Status>> + Send>>;

    async fn chat(
        &self,
        _request: tonic::Request<tonic::Streaming<orders_pb::ChatMessage>>,
    ) -> Result<tonic::Response<Self::ChatStream>, tonic::Status> {
        let outbound = futures_util::stream::iter(::std::iter::empty());
        Ok(tonic::Response::new(Box::pin(outbound)))
    }
}

#[module(providers: [OrdersCounter, PanickingGrpcInterceptor, InterceptorPanicGrpcService])]
struct InterceptorPanicGrpcModule;

async fn boot_guard_panic() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(GuardPanicGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn boot_interceptor_panic() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(InterceptorPanicGrpcModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("BoundAdapters.grpc must be populated").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

/// A panicking guard surfaces as `PermissionDenied` rather than tearing
/// down the connection — matching the "guard said no" semantic. A second
/// call confirms the server stays up across the catch.
#[tokio_localset_test::localset_test]
async fn grpc_panic_in_guard_surfaces_as_permission_denied() {
    let (port, shutdown) = boot_guard_panic().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".into(),
            qty: 1,
        })
        .await
        .expect_err("guard panic must produce an Err — not a connection drop");

    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert!(
        err.message().contains("panicked"),
        "wire message should mention the panic; got {:?}",
        err.message()
    );

    let err2 = client
        .create(orders_pb::CreateOrderRequest {
            item: "again".into(),
            qty: 1,
        })
        .await
        .expect_err("subsequent guard panic must also surface as Err");
    assert_eq!(err2.code(), tonic::Code::PermissionDenied);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}

/// A panicking interceptor surfaces as `Internal` rather than tearing
/// down the connection. The chain runner sets a status on the context;
/// the wrapper reads it and converts to `tonic::Status`.
#[tokio_localset_test::localset_test]
async fn grpc_panic_in_interceptor_surfaces_as_internal() {
    let (port, shutdown) = boot_interceptor_panic().await;
    let mut client = connect(port).await;

    let err = client
        .create(orders_pb::CreateOrderRequest {
            item: "ignored".into(),
            qty: 1,
        })
        .await
        .expect_err("interceptor panic must produce an Err — not a connection drop");

    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(
        err.message().contains("interceptor panicked"),
        "wire message should mention the panic; got {:?}",
        err.message()
    );

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete");
}
