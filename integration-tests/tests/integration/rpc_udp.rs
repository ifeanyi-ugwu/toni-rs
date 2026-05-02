//! End-to-end coverage for the UDP RPC adapter:
//!
//! - request-response round-trips
//! - fire-and-forget (no `id`) does not produce a reply
//! - unknown patterns return a structured error frame
//! - panicking handlers return an error frame instead of hanging
//! - oversized client payloads are rejected before sending
//! - in-flight datagram handlers are awaited during `close()` up to the
//!   configured drain timeout; tasks still running after the timeout are
//!   aborted

use std::time::Duration;

use toni::module;
use toni::rpc::{RpcContext, RpcData, RpcError};
use toni_macros::rpc_controller;

async fn pick_free_udp_port() -> u16 {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    port
}

async fn start_rpc_server(module: toni::module_helpers::module_enum::ModuleDefinition) -> u16 {
    use toni::toni_factory::ToniFactory;
    let port = pick_free_udp_port().await;
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.start().await.unwrap();
    });
    tokio::task::spawn_local(async move { local.await });
    port
}

/// Send one datagram and wait briefly for a reply on the same client socket.
/// Returns `None` if no reply arrives before the deadline.
async fn udp_rpc_timeout(
    port: u16,
    pattern: &str,
    data: serde_json::Value,
    id: Option<&str>,
    deadline: Duration,
) -> Option<serde_json::Value> {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket
        .connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();

    let mut frame = serde_json::json!({"pattern": pattern, "data": data});
    if let Some(id) = id {
        frame["id"] = serde_json::Value::String(id.to_string());
    }

    // The server binds inside its serve future, so the first datagram may be
    // dropped if it lands before bind completes. Retry the send a few times.
    let body = frame.to_string();
    let send_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match socket.send(body.as_bytes()).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < send_deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("UDP send failed: {e}"),
        }
    }

    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(deadline, socket.recv(&mut buf))
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&buf[..n]).ok()
}

#[rpc_controller(pub struct UdpRpcController {})]
impl UdpRpcController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("udp.echo")]
    async fn echo(&self, data: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }

    #[message_pattern("udp.panic")]
    async fn panic_handler(&self, _d: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        panic!("intentional udp rpc panic");
    }

    #[event_pattern("udp.shipped")]
    async fn shipped(&self, _d: RpcData, _c: RpcContext) -> Result<(), RpcError> {
        Ok(())
    }
}

#[module(providers: [UdpRpcController])]
impl UdpRpcModule {}

#[tokio_localset_test::localset_test]
async fn udp_request_response_round_trips() {
    let port = start_rpc_server(UdpRpcModule::module_definition()).await;

    let resp = udp_rpc_timeout(
        port,
        "udp.echo",
        serde_json::json!({"hello": "udp"}),
        Some("1"),
        Duration::from_millis(500),
    )
    .await
    .expect("echo should reply");

    assert_eq!(resp["id"], "1");
    assert_eq!(resp["response"], serde_json::json!({"hello": "udp"}));
}

#[tokio_localset_test::localset_test]
async fn udp_unknown_pattern_returns_error_frame() {
    let port = start_rpc_server(UdpRpcModule::module_definition()).await;

    let resp = udp_rpc_timeout(
        port,
        "udp.does_not_exist",
        serde_json::json!({}),
        Some("1"),
        Duration::from_millis(500),
    )
    .await
    .expect("unknown pattern should reply with an error");

    assert_eq!(resp["id"], "1");
    assert_eq!(resp["err"]["status"], "not_found");
}

#[tokio_localset_test::localset_test]
async fn udp_panicking_handler_returns_error_frame() {
    let port = start_rpc_server(UdpRpcModule::module_definition()).await;

    let resp = udp_rpc_timeout(
        port,
        "udp.panic",
        serde_json::json!({}),
        Some("1"),
        Duration::from_millis(500),
    )
    .await
    .expect("panicking handler must not hang the caller");

    assert_eq!(resp["id"], "1");
    assert!(resp.get("err").is_some(), "expected err frame, got: {resp}");

    // Subsequent request on a fresh socket still succeeds.
    let resp = udp_rpc_timeout(
        port,
        "udp.echo",
        serde_json::json!("ok"),
        Some("2"),
        Duration::from_millis(500),
    )
    .await
    .expect("server should remain responsive after a handler panic");
    assert_eq!(resp["response"], "ok");
}

#[tokio_localset_test::localset_test]
async fn udp_fire_and_forget_produces_no_reply() {
    let port = start_rpc_server(UdpRpcModule::module_definition()).await;

    // No `id` → server must send nothing back.
    let resp = udp_rpc_timeout(
        port,
        "udp.shipped",
        serde_json::json!({"order_id": 42}),
        None,
        Duration::from_millis(200),
    )
    .await;
    assert!(resp.is_none(), "fire-and-forget must not produce a reply");
}

/// `app.shutdown()` must drive the UDP adapter's recv loop to exit, otherwise
/// `shutdown.completed().await` would hang forever. After completion, the
/// socket is closed and the next datagram gets no reply.
#[tokio_localset_test::localset_test]
async fn udp_app_shutdown_stops_the_recv_loop() {
    use toni::toni_factory::ToniFactory;

    let port = pick_free_udp_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(UdpRpcModule::module_definition()).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let shutdown = shutdown_rx.await.unwrap();

    // Server is responsive before shutdown.
    let resp = udp_rpc_timeout(
        port,
        "udp.echo",
        serde_json::json!("hi"),
        Some("1"),
        Duration::from_millis(500),
    )
    .await
    .expect("server should be up");
    assert_eq!(resp["response"], "hi");

    // Trigger shutdown. If the recv loop didn't exit, completed() would hang.
    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete within 2s once close() fires");

    // Subsequent datagrams must not receive a reply.
    let resp = udp_rpc_timeout(
        port,
        "udp.echo",
        serde_json::json!("after"),
        Some("2"),
        Duration::from_millis(200),
    )
    .await;
    assert!(resp.is_none(), "no reply expected after shutdown");
}

/// Spawn a raw UDP responder that drops the first `drop_first` datagrams
/// and echoes each subsequent one back as `{"id":..,"response":<data>}`.
/// Returns the bound port. The server task exits after one echoed reply.
async fn spawn_lossy_echo(drop_first: usize) -> u16 {
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let mut dropped = 0usize;
        loop {
            let (n, src) = server.recv_from(&mut buf).await.unwrap();
            if dropped < drop_first {
                dropped += 1;
                continue;
            }
            let req: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
            let reply = serde_json::json!({
                "id": req["id"],
                "response": req["data"],
            });
            server
                .send_to(reply.to_string().as_bytes(), src)
                .await
                .unwrap();
            return;
        }
    });
    port
}

/// `with_retries(1)` must produce a successful reply when the first datagram
/// is dropped; without retries the same scenario times out.
#[tokio_localset_test::localset_test]
async fn udp_client_retries_recover_from_packet_loss() {
    use toni::{RpcClientTransport, RpcData};

    // No retries → the dropped first datagram is fatal.
    let port_no_retry = spawn_lossy_echo(1).await;
    let no_retry = toni_udp::UdpClientTransport::new("127.0.0.1", port_no_retry)
        .with_timeout(Duration::from_millis(80));
    let err = no_retry
        .send("noop", RpcData::Json(serde_json::json!("hi")))
        .await
        .expect_err("first datagram dropped, no retries → Timeout");
    assert!(matches!(err, toni::RpcClientError::Timeout));

    // One retry → the second datagram gets through.
    let port_retry = spawn_lossy_echo(1).await;
    let retry = toni_udp::UdpClientTransport::new("127.0.0.1", port_retry)
        .with_timeout(Duration::from_millis(80))
        .with_retries(1)
        .with_retry_backoff(Duration::from_millis(20));
    let reply = retry
        .send("noop", RpcData::Json(serde_json::json!("hi")))
        .await
        .expect("retry should recover");
    match reply {
        RpcData::Json(v) => assert_eq!(v, serde_json::json!("hi")),
        other => panic!("unexpected reply: {other:?}"),
    }
}

#[tokio_localset_test::localset_test]
async fn udp_client_transport_round_trips_and_rejects_oversized() {
    use toni::{RpcClientTransport, RpcData};

    let port = start_rpc_server(UdpRpcModule::module_definition()).await;

    // Give the server a moment to bind before the typed client tries to send.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let transport = toni_udp::UdpClientTransport::new("127.0.0.1", port)
        .with_timeout(Duration::from_millis(500));

    let reply = transport
        .send("udp.echo", RpcData::Json(serde_json::json!({"x": 1})))
        .await
        .expect("UdpClientTransport.send should succeed");
    match reply {
        RpcData::Json(v) => assert_eq!(v, serde_json::json!({"x": 1})),
        other => panic!("expected json reply, got {other:?}"),
    }

    // Oversized payload: > 65 507 bytes should be rejected up-front.
    let huge = serde_json::Value::String("a".repeat(70_000));
    let err = transport
        .send("udp.echo", RpcData::Json(huge))
        .await
        .expect_err("oversized payload should fail");
    match err {
        toni::RpcClientError::Transport(msg) => assert!(msg.contains("exceeds")),
        other => panic!("expected Transport error, got {other:?}"),
    }
}

#[rpc_controller(pub struct SlowUdpController {})]
impl SlowUdpController {
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("udp.slow")]
    async fn slow(&self, data: RpcData, _c: RpcContext) -> Result<RpcData, RpcError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(data)
    }
}

#[module(providers: [SlowUdpController])]
impl SlowUdpModule {}

/// A datagram handler already running when shutdown fires must finish
/// during the drain window — its reply must arrive on the client socket.
#[tokio_localset_test::localset_test]
async fn udp_in_flight_request_completes_during_drain() {
    use toni::toni_factory::ToniFactory;

    let port = pick_free_udp_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowUdpModule::module_definition()).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let shutdown = shutdown_rx.await.unwrap();

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client
        .connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let frame =
        serde_json::json!({"pattern":"udp.slow","data":"hi","id":"1"}).to_string();

    // Retry the send until the server is bound.
    let send_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match client.send(frame.as_bytes()).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < send_deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("UDP send failed: {e}"),
        }
    }

    // Give the handler time to enter its sleep before shutdown fires.
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.shutdown();

    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut buf))
        .await
        .expect("in-flight datagram handler must reply during drain")
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(v["response"], "hi");

    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete after drain");
}

/// A datagram handler that outruns the configured drain timeout is aborted.
/// `shutdown.completed()` must resolve promptly even though the handler
/// would otherwise have slept for 300 ms.
#[tokio_localset_test::localset_test]
async fn udp_drain_aborts_after_timeout() {
    use toni::toni_factory::ToniFactory;

    let port = pick_free_udp_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowUdpModule::module_definition()).await;
        let adapter = toni_udp::UdpAdapter::new("127.0.0.1", port)
            .with_drain_timeout(Duration::from_millis(50));
        app.use_rpc_adapter(adapter).unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    let shutdown = shutdown_rx.await.unwrap();

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client
        .connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let frame =
        serde_json::json!({"pattern":"udp.slow","data":"hi","id":"1"}).to_string();
    let send_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match client.send(frame.as_bytes()).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < send_deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("UDP send failed: {e}"),
        }
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    shutdown.shutdown();

    // 50 ms drain budget + slack — must resolve well before the 300 ms handler.
    tokio::time::timeout(Duration::from_millis(500), shutdown.completed())
        .await
        .expect("shutdown must complete after drain timeout aborts the in-flight task");
}
