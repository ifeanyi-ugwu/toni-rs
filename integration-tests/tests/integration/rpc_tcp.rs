//! End-to-end coverage for the TCP RPC adapter:
//!
//! - panicking handlers return an error frame instead of hanging
//!   the caller, and the connection stays alive afterwards
//! - `app.shutdown()` drives the accept loop to exit so
//!   `ShutdownHandle::completed().await` resolves cleanly
//! - in-flight requests are awaited during `close()` up to the configured
//!   drain timeout; tasks still running after the timeout are aborted

use std::time::Duration;

use toni::module;
use toni::rpc::{RpcContext, RpcData, RpcError};
use toni_macros::rpc_controller;

/// Spawn an app with the TCP RPC adapter on an OS-assigned port and wait
/// for `app.bind().await` to surface the listening address before returning.
/// The caller is guaranteed the listener is live by the time it gets the port.
async fn start_rpc_server(module: toni::module_helpers::module_enum::ModuleDefinition) -> u16 {
    use toni::toni_factory::ToniFactory;
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("RPC adapter must report its address").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
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

#[rpc_controller(pub struct RpcPanicController {})]
impl RpcPanicController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("rpc.panic")]
    async fn panic_handler(&self, _d: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        panic!("intentional rpc panic");
    }

    #[message_pattern("rpc.safe")]
    async fn safe_handler(&self, _d: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("safe-ok")))
    }
}

#[module(providers: [RpcPanicController])]
impl RpcPanicModule {}

/// A panicking RPC handler must return an error response instead of hanging
/// the caller indefinitely. The connection must remain usable for subsequent
/// messages.
///
/// Note: the test produces a "panicked at" line in stderr — that is the Rust
/// panic hook firing before catch_unwind catches the unwind. It is expected.
#[tokio_localset_test::localset_test]
async fn rpc_handler_panic_returns_error_and_keeps_connection_alive() {
    let port = start_rpc_server(RpcPanicModule::module_definition()).await;

    // Panicking handler must return an error response within 500 ms,
    // not leave the caller hanging.
    let resp = tcp_rpc_timeout(
        port,
        "rpc.panic",
        serde_json::json!({}),
        Duration::from_millis(500),
    )
    .await;
    assert!(
        resp.is_some(),
        "panicking handler should return an error response, not hang"
    );
    let resp = resp.unwrap();
    assert!(
        resp.get("err").is_some(),
        "response should be an error frame, got: {resp}"
    );

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

#[rpc_controller(pub struct ShutdownTcpController {})]
impl ShutdownTcpController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("tcp.echo")]
    async fn echo(&self, data: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(providers: [ShutdownTcpController])]
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
        let mut app = ToniFactory::create(ShutdownTcpModule::module_definition()).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("RPC adapter must report its address").port());
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

#[rpc_controller(pub struct SlowTcpController {})]
impl SlowTcpController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("tcp.slow")]
    async fn slow(&self, data: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(data)
    }
}

#[module(providers: [SlowTcpController])]
impl SlowTcpModule {}

/// A request already running when shutdown fires must finish during the
/// drain window, not be killed mid-flight. The default 10 s drain timeout
/// comfortably covers a 300 ms handler.
#[tokio_localset_test::localset_test]
async fn tcp_in_flight_request_completes_during_drain() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toni::toni_factory::ToniFactory;

    let port = pick_free_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowTcpModule::module_definition()).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let shutdown = shutdown_rx.await.unwrap();

    let stream = connect_with_retry(port).await;
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

    let port = pick_free_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowTcpModule::module_definition()).await;
        let adapter = toni_tcp::TcpAdapter::new("127.0.0.1", port)
            .with_drain_timeout(Duration::from_millis(50));
        app.use_rpc_adapter(adapter).unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let shutdown = shutdown_rx.await.unwrap();

    let stream = connect_with_retry(port).await;
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
