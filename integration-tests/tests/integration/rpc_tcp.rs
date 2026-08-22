//! End-to-end coverage for the TCP RPC adapter:
//!
//! - panicking handlers return an error frame instead of hanging
//!   the caller, and the connection stays alive afterwards
//! - `app.shutdown()` drives the accept loop to exit so
//!   `ShutdownHandle::completed().await` resolves cleanly
//! - in-flight requests are awaited during `close()` up to the configured
//!   drain timeout; tasks still running after the timeout are aborted
//! - inbound requests that would exceed `with_max_inflight` are rejected
//!   with an `"overloaded"` frame and the slot is released when the
//!   in-flight handler completes

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use toni::rpc::RpcHandlerResult;

use serde::{Deserialize, Serialize};
use toni::async_trait;
use toni::context::{Extensions, HandlerContext, RpcContext};
use toni::errors::{ErrorKind, PanicRecovered, PipelineSegment};
use toni::extractors::Payload;
use toni::injectable;
use toni::module;
use toni::rpc::{RpcData, RpcError};
use toni::traits_helpers::{
    ChainError, ErrorHandler, ErrorObserver, Guard, Interceptor, InterceptorNext, Pipe,
};
use toni_macros::{new, patterns, rpc_controller, set_metadata};

/// Spawn an app with the TCP RPC adapter on an OS-assigned port and wait
/// for `app.bind().await` to surface the listening address before returning.
/// The caller is guaranteed the listener is live by the time it gets the port.
async fn start_rpc_server(module: impl toni::ModuleMetadata + 'static) -> u16 {
    start_rpc_server_with_observers(module, vec![]).await
}

/// Spawn an app with the TCP RPC adapter on an OS-assigned port and
/// register the supplied global error observers before bootstrap.
async fn start_rpc_server_with_observers(
    module: impl toni::ModuleMetadata + 'static,
    observers: Vec<Arc<dyn ErrorObserver>>,
) -> u16 {
    use toni::toni_factory::ToniFactory;
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        for o in observers {
            factory.use_global_error_observer(o);
        }
        let mut app = factory.create_with(module).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(
            bound
                .rpc
                .expect("RPC adapter must report its address")
                .port(),
        );
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

/// Captures the most recent `PanicRecovered.during` seen by the global
/// error observer chain.
struct RpcSegmentObserver {
    count: Arc<AtomicUsize>,
    captured: Arc<std::sync::Mutex<Option<PipelineSegment>>>,
}

#[async_trait]
impl ErrorObserver for RpcSegmentObserver {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn std::error::Error + Send + Sync + 'static),
        _ctx: &'a (dyn toni::context::HandlerContext + 'a),
    ) {
        self.count.fetch_add(1, Ordering::SeqCst);
        if let Some(p) = error.downcast_ref::<PanicRecovered>() {
            *self.captured.lock().unwrap() = Some(p.during);
        }
    }
}

/// Sends one request over a raw TCP connection with a timeout.
/// Returns None if no response arrives within the deadline.
async fn tcp_rpc_timeout(
    port: u16,
    pattern: &str,
    data: serde_json::Value,
    deadline: Duration,
) -> Option<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    let _ = tokio::time::timeout(deadline, reader.read_line(&mut line))
        .await
        .ok()?;
    serde_json::from_str(line.trim()).ok()
}

#[rpc_controller]
pub struct RpcPanicController {}
#[patterns]
impl RpcPanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.panic")]
    async fn panic_handler(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        panic!("intentional rpc panic");
    }

    #[message_pattern("rpc.safe")]
    async fn safe_handler(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("safe-ok")))
    }
}

#[module(controllers: [RpcPanicController])]
impl RpcPanicModule {}

/// A panicking RPC handler is caught by the dispatcher, surfaced as a
/// `PanicRecovered` framework event, and rendered through
/// `RpcError::to_data`. The reply is a canonical-envelope success
/// frame (not a wire-Err) and the connection stays usable for subsequent
/// messages.
///
/// Note: the test produces a "panicked at" line in stderr — that is the Rust
/// panic hook firing before catch_unwind catches the unwind. It is expected.
#[tokio_localset_test::localset_test]
async fn rpc_handler_panic_returns_error_and_keeps_connection_alive() {
    let port = start_rpc_server(RpcPanicModule).await;

    // Panicking handler must return a response within 500 ms, not hang.
    let resp = tcp_rpc_timeout(
        port,
        "rpc.panic",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("panicking handler should return a response, not hang");
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");

    // Connection must still be usable — safe handler works on a fresh connection.
    let resp = tcp_rpc_timeout(
        port,
        "rpc.safe",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await;
    assert_eq!(resp.unwrap()["response"], "safe-ok");
}

#[rpc_controller]
pub struct ShutdownTcpController {}
#[patterns]
impl ShutdownTcpController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("tcp.echo")]
    async fn echo(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [ShutdownTcpController])]
impl ShutdownTcpModule {}

/// `app.shutdown()` must drive the accept loop to exit; otherwise
/// `shutdown.completed().await` would hang forever. After completion, new
/// connections to the listener are refused.
#[tokio_localset_test::localset_test]
async fn tcp_app_shutdown_stops_the_accept_loop() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ShutdownTcpModule).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(
            bound
                .rpc
                .expect("RPC adapter must report its address")
                .port(),
        );
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    // Server is responsive before shutdown.
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame = serde_json::json!({"pattern":"tcp.echo","data":"hi","id":"1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line))
        .await
        .expect("read should not time out")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["response"], "hi");

    // Trigger shutdown. If the accept loop didn't exit, completed() would hang.
    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete within 2s once close() fires");

    // Listener is closed — new connections are refused.
    let connect = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(
        connect.is_err(),
        "listener should be closed after shutdown, got {connect:?}"
    );
}

#[rpc_controller]
pub struct SlowTcpController {}
#[patterns]
impl SlowTcpController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("tcp.slow")]
    async fn slow(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(data)
    }
}

#[module(controllers: [SlowTcpController])]
impl SlowTcpModule {}

/// A request already running when shutdown fires must finish during the
/// drain window, not be killed mid-flight. The default 10 s drain timeout
/// comfortably covers a 300 ms handler.
#[tokio_localset_test::localset_test]
async fn tcp_in_flight_request_completes_during_drain() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowTcpModule).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(
            bound
                .rpc
                .expect("RPC adapter must report its address")
                .port(),
        );
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern":"tcp.slow","data":"hi","id":"1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    // Give the handler time to enter its sleep before shutdown fires.
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.shutdown();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("in-flight request must complete during drain")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["response"], "hi");

    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete after drain");
}

/// When a handler outruns the configured drain timeout, the framework aborts
/// it instead of waiting forever. The caller doesn't get a reply (the task is
/// killed mid-flight) but `shutdown.completed()` resolves promptly.
#[tokio_localset_test::localset_test]
async fn tcp_drain_aborts_after_timeout() {
    use tokio::io::AsyncWriteExt;
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowTcpModule).await;
        let adapter =
            toni_tcp::TcpAdapter::new("127.0.0.1", 0).with_drain_timeout(Duration::from_millis(50));
        app.use_rpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(
            bound
                .rpc
                .expect("RPC adapter must report its address")
                .port(),
        );
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (_reader, mut writer) = stream.into_split();
    // Handler sleeps 300 ms but the drain budget is only 50 ms.
    let mut frame = serde_json::json!({"pattern":"tcp.slow","data":"hi","id":"1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    shutdown.shutdown();

    // 50 ms drain + slack — must finish well before the 300 ms handler would.
    tokio::time::timeout(Duration::from_millis(500), shutdown.completed())
        .await
        .expect("shutdown must complete after drain timeout aborts the in-flight task");
}

/// With `with_max_inflight(1)` and a slow handler holding the only slot, a
/// concurrent request on a second connection must be rejected immediately
/// with an `"overloaded"` frame rather than queuing. After the slow handler
/// completes the slot is released and a follow-up request succeeds.
#[tokio_localset_test::localset_test]
async fn tcp_backpressure_rejects_excess_and_releases_after_completion() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowTcpModule).await;
        let adapter = toni_tcp::TcpAdapter::new("127.0.0.1", 0).with_max_inflight(1);
        app.use_rpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(
            bound
                .rpc
                .expect("RPC adapter must report its address")
                .port(),
        );
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let port = port_rx.await.unwrap();

    // Connection 1: occupy the only slot with a slow handler.
    let stream1 = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader1, mut writer1) = stream1.into_split();
    let mut reader1 = BufReader::new(reader1);
    let mut frame = serde_json::json!({"pattern":"tcp.slow","data":"first","id":"1"}).to_string();
    frame.push('\n');
    writer1.write_all(frame.as_bytes()).await.unwrap();
    // Give the server time to spawn the handler so the slot is genuinely held.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connection 2: should be rejected with "overloaded" since the slot is full.
    let stream2 = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader2, mut writer2) = stream2.into_split();
    let mut reader2 = BufReader::new(reader2);
    let mut frame = serde_json::json!({"pattern":"tcp.slow","data":"second","id":"2"}).to_string();
    frame.push('\n');
    writer2.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_millis(200), reader2.read_line(&mut line))
        .await
        .expect("rejection should arrive immediately, not queue")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["id"], "2");
    assert_eq!(v["err"]["status"], "overloaded");

    // Wait for the slow handler on connection 1 to finish.
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader1.read_line(&mut line))
        .await
        .expect("first handler should reply within 2s")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["response"], "first");

    // Slot is now free — a fresh request on connection 2 succeeds.
    let mut frame = serde_json::json!({"pattern":"tcp.slow","data":"third","id":"3"}).to_string();
    frame.push('\n');
    writer2.write_all(frame.as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader2.read_line(&mut line))
        .await
        .expect("third handler should reply after slot is freed")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["response"], "third");
}

// ---- Typed-payload coverage --------------------------------------------------
//
// The macro emits two distinct payload-extraction shapes: `data` for handlers
// that take raw `RpcData`, and `data.parse::<T>()` for typed DTOs. Earlier
// transitions had only `RpcData` coverage in tests, so changes that broke the
// typed path slipped past CI. These tests exercise the typed-DTO path
// explicitly.

#[derive(Debug, Deserialize)]
struct EchoDto {
    text: String,
    count: u32,
}

#[derive(Debug, Serialize)]
struct EchoReply {
    repeated: String,
}

#[rpc_controller]
pub struct TypedPayloadController {}
#[patterns]
impl TypedPayloadController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("typed.echo")]
    async fn echo(&self, payload: EchoDto, _ctx: &RpcContext) -> Result<EchoReply, RpcError> {
        Ok(EchoReply {
            repeated: payload.text.repeat(payload.count as usize),
        })
    }
}

#[module(controllers: [TypedPayloadController])]
impl TypedPayloadModule {}

#[tokio_localset_test::localset_test]
async fn typed_payload_round_trip_succeeds() {
    let port = start_rpc_server(TypedPayloadModule).await;
    let resp = tcp_rpc_timeout(
        port,
        "typed.echo",
        serde_json::json!({"text": "ab", "count": 3}),
        Duration::from_secs(1),
    )
    .await
    .expect("typed echo response");
    assert_eq!(resp["response"]["repeated"], "ababab");
}

#[tokio_localset_test::localset_test]
async fn typed_payload_parse_failure_renders_canonical_envelope() {
    // Exercises the macro's typed-payload parse-error path: deserialise
    // failure renders through `RpcError::to_data` rather than
    // surfacing as a wire-level Err frame.
    let port = start_rpc_server(TypedPayloadModule).await;
    let resp = tcp_rpc_timeout(
        port,
        "typed.echo",
        serde_json::json!({"wrong": "shape"}),
        Duration::from_secs(1),
    )
    .await
    .expect("parse-error response");
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");
}

#[injectable]
pub struct PanickingRpcGuard {}
impl PanickingRpcGuard {}

#[async_trait]
impl Guard<RpcContext> for PanickingRpcGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        panic!("rpc guard kaboom");
    }
}

#[rpc_controller]
pub struct RpcGuardPanicController {}
#[patterns]
impl RpcGuardPanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.guarded")]
    #[use_guards(PanickingRpcGuard)]
    async fn guarded(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("unreachable")))
    }

    #[message_pattern("rpc.safe")]
    async fn safe(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("safe-ok")))
    }
}

#[module(controllers: [RpcGuardPanicController], providers: [PanickingRpcGuard])]
impl RpcGuardPanicModule {}

#[tokio_localset_test::localset_test]
async fn rpc_guard_panic_surfaces_as_forbidden_and_keeps_connection_alive() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(RpcSegmentObserver {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_rpc_server_with_observers(RpcGuardPanicModule, vec![observer]).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.guarded",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("guard panic must produce a reply, not hang");
    assert_eq!(resp["err"]["status"], "forbidden");
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Guard));

    let resp = tcp_rpc_timeout(
        port,
        "rpc.safe",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("safe handler should reply");
    assert_eq!(resp["response"], "safe-ok");
}

#[injectable]
pub struct PanickingRpcInterceptor {}
impl PanickingRpcInterceptor {}

#[async_trait]
impl Interceptor<RpcContext, RpcHandlerResult> for PanickingRpcInterceptor {
    async fn intercept(
        &self,
        _ctx: &RpcContext,
        _next: Box<dyn InterceptorNext<RpcContext, RpcHandlerResult>>,
    ) -> RpcHandlerResult {
        panic!("rpc interceptor kaboom");
    }
}

#[rpc_controller]
pub struct RpcInterceptorPanicController {}
#[patterns]
impl RpcInterceptorPanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.intercepted")]
    #[use_interceptors(PanickingRpcInterceptor)]
    async fn intercepted(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("unreachable")))
    }

    #[message_pattern("rpc.safe")]
    async fn safe(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("safe-ok")))
    }
}

#[module(controllers: [RpcInterceptorPanicController], providers: [PanickingRpcInterceptor])]
impl RpcInterceptorPanicModule {}

#[tokio_localset_test::localset_test]
async fn rpc_interceptor_panic_surfaces_as_envelope_and_keeps_connection_alive() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(RpcSegmentObserver {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_rpc_server_with_observers(RpcInterceptorPanicModule, vec![observer]).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.intercepted",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("interceptor panic must produce a reply, not hang");
    // Interceptor panic stashes `Err(RpcError::Internal)` on the context;
    // adapter renders it as a wire-error frame.
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Middleware));

    let resp = tcp_rpc_timeout(
        port,
        "rpc.safe",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("safe handler should reply");
    assert_eq!(resp["response"], "safe-ok");
}

#[injectable]
pub struct PanickingRpcPipe {}
impl PanickingRpcPipe {}

impl Pipe<RpcContext, RpcHandlerResult> for PanickingRpcPipe {
    fn process(&self, _ctx: &RpcContext) -> Option<RpcHandlerResult> {
        panic!("rpc pipe kaboom");
        None
    }
}

#[rpc_controller]
pub struct RpcPipePanicController {}
#[patterns]
impl RpcPipePanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.piped")]
    #[use_pipes(PanickingRpcPipe)]
    async fn piped(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("unreachable")))
    }

    #[message_pattern("rpc.safe")]
    async fn safe(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("safe-ok")))
    }
}

#[module(controllers: [RpcPipePanicController], providers: [PanickingRpcPipe])]
impl RpcPipePanicModule {}

#[tokio_localset_test::localset_test]
async fn rpc_pipe_panic_surfaces_as_envelope_and_keeps_connection_alive() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(RpcSegmentObserver {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_rpc_server_with_observers(RpcPipePanicModule, vec![observer]).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.piped",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("pipe panic must produce a reply, not hang");
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Pipe));

    let resp = tcp_rpc_timeout(
        port,
        "rpc.safe",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("safe handler should reply");
    assert_eq!(resp["response"], "safe-ok");
}

#[injectable]
pub struct PanickingRpcErrorHandler {}
impl PanickingRpcErrorHandler {}

#[async_trait]
impl ErrorHandler<RpcContext, RpcData> for PanickingRpcErrorHandler {
    async fn handle_error(&self, _error: ChainError<'_>, _ctx: &RpcContext) -> Option<RpcData> {
        panic!("rpc error-handler kaboom");
    }
}

#[rpc_controller]
pub struct RpcErrorHandlerPanicController {}
#[patterns]
impl RpcErrorHandlerPanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.eh")]
    #[use_error_handlers(PanickingRpcErrorHandler)]
    async fn eh(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        panic!("handler kaboom");
    }
}

#[module(controllers: [RpcErrorHandlerPanicController], providers: [PanickingRpcErrorHandler])]
impl RpcErrorHandlerPanicModule {}

#[tokio_localset_test::localset_test]
async fn rpc_error_handler_panic_continues_chain_to_default_rendering() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(RpcSegmentObserver {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_rpc_server_with_observers(RpcErrorHandlerPanicModule, vec![observer]).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.eh",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("error-handler panic must not break the chain");
    // The chain skipped the panicking handler and fell through to the
    // default rendering — which surfaces the original handler panic.
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");

    // Observer fired twice (HandlerBody first, then ErrorHandler); the
    // captured segment is the most recent `PanicRecovered`.
    assert!(count.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        *captured.lock().unwrap(),
        Some(PipelineSegment::ErrorHandler)
    );
}

/// Domain error whose `message()` panics — exercises the renderer-panic
/// fallback path of the RPC dispatcher.
#[derive(Debug)]
struct RpcRenderBomb;

impl std::fmt::Display for RpcRenderBomb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcRenderBomb")
    }
}

impl std::error::Error for RpcRenderBomb {}

impl toni::Error for RpcRenderBomb {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }
    fn message(&self) -> std::borrow::Cow<'_, str> {
        panic!("rpc render kaboom");
    }
}

#[rpc_controller]
pub struct RpcRenderPanicController {}
#[patterns]
impl RpcRenderPanicController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.render")]
    async fn render(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Err(RpcError::from(RpcRenderBomb))
    }
}

#[module(controllers: [RpcRenderPanicController])]
impl RpcRenderPanicModule {}

#[tokio_localset_test::localset_test]
async fn rpc_renderer_panic_falls_back_to_safe_envelope() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(RpcSegmentObserver {
        count: count.clone(),
        captured: captured.clone(),
    });
    let port = start_rpc_server_with_observers(RpcRenderPanicModule, vec![observer]).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.render",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("renderer panic must produce a fallback envelope, not hang");
    // Fallback envelope is the hardcoded `RpcData::text("Internal Server Error")`.
    assert_eq!(resp["response"], "Internal Server Error");
    assert_eq!(
        *captured.lock().unwrap(),
        Some(PipelineSegment::ResponseRendering),
    );
}

// ---- Metadata round-trip -----------------------------------------------------
//
// Metadata set on the client builder must ride the TCP frame's `metadata`
// field and surface in the handler's RpcContext.

#[rpc_controller]
pub struct TcpMetaController {}
#[patterns]
impl TcpMetaController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("meta.echo")]
    async fn meta_echo(&self, _d: RpcData, c: &RpcContext) -> Result<RpcData, RpcError> {
        let trace = c.header("trace").unwrap_or("none").to_string();
        Ok(RpcData::json(serde_json::json!({ "trace": trace })))
    }
}

#[module(controllers: [TcpMetaController])]
impl TcpMetaModule {}

#[tokio_localset_test::localset_test]
async fn tcp_client_metadata_reaches_handler() {
    use toni::RpcClient;

    let port = start_rpc_server(TcpMetaModule).await;
    let client = RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));

    let resp = client
        .request("meta.echo")
        .header("trace", "abc123")
        .send(RpcData::json(serde_json::json!({})))
        .await
        .expect("metadata request should round-trip");
    assert_eq!(
        resp.as_json().and_then(|v| v["trace"].as_str()),
        Some("abc123"),
        "client metadata must reach the handler over TCP"
    );
}

// A `#[rpc_controller]` with no `#[patterns]` impl: the `RpcHandlersBridge` defaults answer, so it
// is a complete provider that routes no patterns. The absence of an impl block is the point.
#[rpc_controller]
pub struct BareRpcController {}

#[module(controllers: [BareRpcController])]
impl BareRpcModule {}

/// The self-sufficiency guarantee: `#[rpc_controller]` alone is valid and `#[patterns]` only adds
/// handlers. An app whose only controller is bare starts, and every pattern is unrouted — the
/// bridge's empty pattern list leaves the dispatch index with nothing in it.
#[tokio_localset_test::localset_test]
async fn bare_rpc_controller_registers_with_no_patterns() {
    let port = start_rpc_server(BareRpcModule).await;

    let resp = tcp_rpc_timeout(
        port,
        "anything",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("a bare controller must still leave a serving app");
    assert_eq!(
        resp["err"]["status"], "not_found",
        "a controller with no #[patterns] impl routes nothing"
    );
}

#[derive(Clone)]
pub struct RpcPrincipal(String);

#[injectable]
pub struct RpcAuthGuard {}

#[async_trait]
impl Guard<RpcContext> for RpcAuthGuard {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.extensions().insert(RpcPrincipal("carol".into()));
        true
    }
}

#[rpc_controller]
pub struct RpcBusController {}

#[patterns]
impl RpcBusController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.bus")]
    #[use_guards(RpcAuthGuard)]
    /// Takes the context exclusively — the form every transport's handler uses.
    /// The `&RpcContext` handlers elsewhere in this file still compile, since
    /// `&mut` coerces at the call site.
    async fn bus(&self, _d: RpcData, ctx: &RpcContext) -> Result<RpcData, RpcError> {
        let who = ctx
            .extensions()
            .get::<RpcPrincipal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "ABSENT".into());
        Ok(RpcData::json(serde_json::json!(who)))
    }
}

#[module(controllers: [RpcBusController], providers: [RpcAuthGuard])]
impl RpcBusModule {}

/// The RPC handler takes `&RpcContext`, so it reads the guard's write straight
/// off the context. `extension_bus.rs` covers HTTP and WebSocket, whose handlers
/// reach the same bag by other routes.
#[tokio_localset_test::localset_test]
async fn rpc_guard_write_reaches_the_handler() {
    let port = start_rpc_server(RpcBusModule).await;
    let resp = tcp_rpc_timeout(
        port,
        "rpc.bus",
        serde_json::json!({}),
        Duration::from_secs(1),
    )
    .await
    .expect("bus response");

    assert_eq!(resp["response"], "carol");
}

// ── handler parameter shapes ────────────────────────────────────────────────
//
// An RPC handler used to take exactly `(payload, ctx)`. It now takes what it
// needs: the handlers above still declare the old pair and are untouched, which
// is the compatibility half of the same change.

#[derive(Clone, Deserialize, Serialize)]
pub struct PlaceOrder {
    item: String,
    qty: u32,
}

#[derive(Clone)]
pub struct Caller(String);

#[injectable]
pub struct ShapeGuard {}

#[async_trait]
impl Guard<RpcContext> for ShapeGuard {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.extensions().insert(Caller("frank".into()));
        true
    }
}

#[rpc_controller]
pub struct ShapeController {}

#[patterns]
#[use_guards(ShapeGuard)]
impl ShapeController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The payload alone, wrapped — no context parameter at all.
    #[message_pattern("shape.payload")]
    async fn payload_only(&self, Payload(order): Payload<PlaceOrder>) -> Result<String, RpcError> {
        Ok(format!("{}x{}", order.item, order.qty))
    }

    /// Extractors in an order the fixed pair could not express, and the raw data
    /// beside the parsed form of it.
    #[message_pattern("shape.mixed")]
    async fn mixed(
        &self,
        ext: Extensions,
        ctx: &RpcContext,
        raw: RpcData,
    ) -> Result<String, RpcError> {
        let caller = ext
            .get::<Caller>()
            .map(|c| c.0)
            .unwrap_or_else(|| "ABSENT".into());
        Ok(format!(
            "{caller}/{}/{}",
            ctx.pattern(),
            raw.parse::<PlaceOrder>().is_ok()
        ))
    }

    /// Nothing at all.
    #[message_pattern("shape.nothing")]
    async fn nothing(&self) -> Result<String, RpcError> {
        Ok("ok".to_string())
    }
}

#[module(controllers: [ShapeController], providers: [ShapeGuard])]
impl ShapeModule {}

async fn shape_call(port: u16, pattern: &str) -> serde_json::Value {
    tcp_rpc_timeout(
        port,
        pattern,
        serde_json::json!({"item": "boots", "qty": 2}),
        Duration::from_secs(1),
    )
    .await
    .expect("response")["response"]
        .clone()
}

#[tokio_localset_test::localset_test]
async fn a_handler_takes_only_the_payload() {
    let port = start_rpc_server(ShapeModule).await;
    assert_eq!(shape_call(port, "shape.payload").await, "bootsx2");
}

#[tokio_localset_test::localset_test]
async fn rpc_extractors_compose_in_any_order() {
    let port = start_rpc_server(ShapeModule).await;
    assert_eq!(
        shape_call(port, "shape.mixed").await,
        "frank/shape.mixed/true"
    );
}

#[tokio_localset_test::localset_test]
async fn an_rpc_handler_can_take_nothing() {
    let port = start_rpc_server(ShapeModule).await;
    assert_eq!(shape_call(port, "shape.nothing").await, "ok");
}

// ---- request scope on a transport that is not HTTP ---------------------------

/// Counts how many times it is constructed, process-wide.
static SCOPED_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// Request-scoped, so one construction per execution and no more — the property
/// that used to be reachable only on HTTP.
#[injectable(scope = "request")]
pub struct PerCall {
    #[default(0)]
    id: usize,
}

impl PerCall {
    #[new]
    pub fn new() -> Self {
        Self {
            id: SCOPED_BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    /// Which construction this instance came from. Two holders reporting the
    /// same number are holding one instance.
    pub fn id(&self) -> usize {
        self.id
    }
}

/// Resolves `PerCall` twice through two separate guards plus the handler. All
/// three must see one instance.
#[injectable(scope = "request")]
pub struct ScopedGuardA {
    #[inject]
    scoped: PerCall,
}

#[async_trait]
impl Guard<RpcContext> for ScopedGuardA {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.extensions().insert(SeenA(self.scoped.id()));
        true
    }
}

#[injectable(scope = "request")]
pub struct ScopedGuardB {
    #[inject]
    scoped: PerCall,
}

#[async_trait]
impl Guard<RpcContext> for ScopedGuardB {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.extensions().insert(SeenB(self.scoped.id()));
        true
    }
}

#[derive(Clone)]
pub struct SeenA(usize);
#[derive(Clone)]
pub struct SeenB(usize);

#[rpc_controller]
pub struct ScopedRpcController {}

#[patterns]
#[use_guards(ScopedGuardA, ScopedGuardB)]
impl ScopedRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.scoped")]
    async fn scoped(&self, _d: RpcData, ctx: &RpcContext) -> Result<RpcData, RpcError> {
        let a = ctx.extensions().get::<SeenA>().map(|s| s.0).unwrap_or(0);
        let b = ctx.extensions().get::<SeenB>().map(|s| s.0).unwrap_or(0);
        Ok(RpcData::json(serde_json::json!({
            "a": a,
            "b": b,
            "builds": SCOPED_BUILDS.load(Ordering::SeqCst),
        })))
    }
}

#[module(controllers: [ScopedRpcController], providers: [PerCall, ScopedGuardA, ScopedGuardB])]
impl ScopedRpcModule {}

/// A request-scoped provider injected into two RPC guards is constructed once
/// per call and shared, exactly as on HTTP.
///
/// Registering this at all used to be refused at startup — the RPC resolver
/// rejected a factory enhancer with request-scoped dependencies on the grounds
/// that "RPC has no HTTP request context". Every transport carries an execution
/// now, and the cache that makes the scope mean anything lives on it.
#[tokio_localset_test::localset_test]
async fn a_request_scoped_provider_is_shared_within_one_rpc_call() {
    SCOPED_BUILDS.store(0, Ordering::SeqCst);
    let port = start_rpc_server(ScopedRpcModule).await;

    let resp = tcp_rpc_timeout(
        port,
        "rpc.scoped",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("scoped handler should answer");

    // Both guards resolved it; one construction served them both.
    assert_eq!(
        resp["response"]["builds"], 1,
        "one execution builds it once: {resp}"
    );
    assert_eq!(
        resp["response"]["a"], resp["response"]["b"],
        "both guards hold the same instance: {resp}"
    );
}

// ---- a controller built per call --------------------------------------------

/// Hands out a distinct id per construction, so two holders reporting the same
/// number are holding one instance. Separate from `SCOPED_BUILDS` because these
/// tests run concurrently and that counter is reset by its own test.
static CALL_IDS: AtomicUsize = AtomicUsize::new(0);

#[injectable(scope = "request")]
pub struct CallScoped {
    #[default(0)]
    id: usize,
}

impl CallScoped {
    #[new]
    pub fn new() -> Self {
        Self {
            id: CALL_IDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }
}

#[derive(Clone)]
pub struct GuardSaw(usize);

/// Reads `CallScoped` before the controller exists, so the id it records is the
/// one the execution already holds by the time the controller is built.
#[injectable(scope = "request")]
pub struct CallScopedGuard {
    #[inject]
    scoped: CallScoped,
}

#[async_trait]
impl Guard<RpcContext> for CallScopedGuard {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.extensions().insert(GuardSaw(self.scoped.id()));
        true
    }
}

/// Numbers each controller construction.
static PER_CALL_CONTROLLER_BUILDS: AtomicUsize = AtomicUsize::new(0);

#[rpc_controller(scope = "request")]
pub struct PerCallRpcController {
    #[inject]
    scoped: CallScoped,
    #[default(0)]
    build: usize,
}

#[patterns]
#[use_guards(CallScopedGuard)]
impl PerCallRpcController {
    #[new]
    pub fn new(scoped: CallScoped) -> Self {
        Self {
            scoped,
            build: PER_CALL_CONTROLLER_BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    #[message_pattern("rpc.per_call")]
    async fn per_call(&self, _d: RpcData, ctx: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!({
            "build": self.build,
            "controller_saw": self.scoped.id(),
            "guard_saw": ctx.extensions().get::<GuardSaw>().map(|s| s.0),
        })))
    }
}

#[module(controllers: [PerCallRpcController], providers: [CallScoped, CallScopedGuard])]
impl PerCallRpcModule {}

/// Numbers each construction of the singleton controller below.
static SINGLETON_CONTROLLER_BUILDS: AtomicUsize = AtomicUsize::new(0);

#[rpc_controller]
pub struct SingletonRpcController {
    #[default(0)]
    build: usize,
}

#[patterns]
impl SingletonRpcController {
    #[new]
    pub fn new() -> Self {
        Self {
            build: SINGLETON_CONTROLLER_BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    #[message_pattern("rpc.singleton")]
    async fn singleton(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!({ "build": self.build })))
    }
}

#[module(controllers: [SingletonRpcController])]
impl SingletonRpcModule {}

/// `#[rpc_controller(scope = "request")]` builds the controller inside the call it
/// serves: a fresh one per message, and its request-scoped dependency is the
/// instance the call already holds rather than a second one.
#[tokio_localset_test::localset_test]
async fn a_request_scoped_rpc_controller_is_built_per_call() {
    let port = start_rpc_server(PerCallRpcModule).await;

    let first = tcp_rpc_timeout(
        port,
        "rpc.per_call",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("per-call controller should answer")["response"]
        .clone();
    let second = tcp_rpc_timeout(
        port,
        "rpc.per_call",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("per-call controller should answer again")["response"]
        .clone();

    assert_ne!(
        first["build"], second["build"],
        "each call builds its own controller: {first} then {second}"
    );
    // The guard ran first and resolved `CallScoped` before the controller existed.
    // Equal ids mean the controller joined that same execution rather than starting one.
    assert_eq!(
        first["controller_saw"], first["guard_saw"],
        "the controller shares the call's request-scoped instance: {first}"
    );
    assert_eq!(
        second["controller_saw"], second["guard_saw"],
        "the controller shares the call's request-scoped instance: {second}"
    );
    assert_ne!(
        first["controller_saw"], second["controller_saw"],
        "a second call gets its own instance: {first} then {second}"
    );
}

/// The default is unchanged: a controller with no declared scope is built once at
/// startup and every call is served by that one instance.
#[tokio_localset_test::localset_test]
async fn a_singleton_rpc_controller_is_built_once() {
    let port = start_rpc_server(SingletonRpcModule).await;

    let first = tcp_rpc_timeout(
        port,
        "rpc.singleton",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("singleton controller should answer")["response"]["build"]
        .clone();
    let second = tcp_rpc_timeout(
        port,
        "rpc.singleton",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("singleton controller should answer again")["response"]["build"]
        .clone();

    assert_eq!(first, second, "one instance serves every call");
}

/// Numbers each construction of the controller that never declares a scope.
static ELEVATED_CONTROLLER_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// Declares no scope, and depends on a request-scoped provider.
#[rpc_controller]
pub struct ElevatedRpcController {
    #[inject]
    scoped: CallScoped,
    #[default(0)]
    build: usize,
}

#[patterns]
impl ElevatedRpcController {
    #[new]
    pub fn new(scoped: CallScoped) -> Self {
        Self {
            scoped,
            build: ELEVATED_CONTROLLER_BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    #[message_pattern("rpc.elevated")]
    async fn elevated(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!({
            "build": self.build,
            "saw": self.scoped.id(),
        })))
    }
}

#[module(controllers: [ElevatedRpcController], providers: [CallScoped])]
impl ElevatedRpcModule {}

/// A controller that declares no scope but depends on a request-scoped provider is elevated rather
/// than refused. Registering this used to abort at startup: a singleton cannot hold something that
/// belongs to one call, and `#[rpc_controller]` had no other scope to offer.
#[tokio_localset_test::localset_test]
async fn an_rpc_controller_elevates_to_request_scope() {
    let port = start_rpc_server(ElevatedRpcModule).await;

    let first = tcp_rpc_timeout(
        port,
        "rpc.elevated",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("elevated controller should answer")["response"]
        .clone();
    let second = tcp_rpc_timeout(
        port,
        "rpc.elevated",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await
    .expect("elevated controller should answer again")["response"]
        .clone();

    assert_ne!(
        first["build"], second["build"],
        "elevation means one controller per call: {first} then {second}"
    );
    assert_ne!(
        first["saw"], second["saw"],
        "and its request-scoped dependency is the calling execution's: {first} then {second}"
    );
}

// ---- declared metadata -------------------------------------------------------

#[derive(Clone)]
pub struct Tier(&'static str);

#[derive(Clone)]
pub struct Audience(&'static str);

#[rpc_controller]
pub struct MetaRpcController {}

/// Both entries apply to every pattern below unless one overrides them.
#[patterns]
#[set_metadata(Tier("standard"))]
#[set_metadata(Audience("internal"))]
impl MetaRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("meta.inherited")]
    async fn inherited(&self, ctx: &RpcContext) -> Result<String, RpcError> {
        Ok(read_declared(ctx))
    }

    #[message_pattern("meta.overridden")]
    #[set_metadata(Tier("premium"))]
    async fn overridden(&self, ctx: &RpcContext) -> Result<String, RpcError> {
        Ok(read_declared(ctx))
    }
}

fn read_declared(ctx: &RpcContext) -> String {
    let m = ctx.metadata();
    let tier = m
        .and_then(|m| m.get::<Tier>())
        .map(|t| t.0)
        .unwrap_or("none");
    let audience = m
        .and_then(|m| m.get::<Audience>())
        .map(|a| a.0)
        .unwrap_or("none");
    format!("{tier}/{audience}")
}

#[module(controllers: [MetaRpcController])]
impl MetaRpcModule {}

/// `#[set_metadata]` reaches a transport that populated nothing before: the controller's entries
/// apply to every pattern, and a pattern that declares its own shadows the matching type.
#[tokio_localset_test::localset_test]
async fn declared_metadata_reaches_an_rpc_handler() {
    let port = start_rpc_server(MetaRpcModule).await;
    assert_eq!(
        shape_call(port, "meta.inherited").await,
        "standard/internal"
    );
    assert_eq!(
        shape_call(port, "meta.overridden").await,
        "premium/internal"
    );
}
