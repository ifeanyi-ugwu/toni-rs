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
//! - inbound datagrams that would exceed `with_max_inflight` are rejected
//!   with an `"overloaded"` frame and the slot is released when the
//!   in-flight handler completes

use std::time::Duration;

use toni::context::RpcContext;
use toni::module;
use toni::rpc::{RpcData, RpcError};
use toni_macros::{new, patterns, rpc_controller};

/// Spawn an app with the UDP RPC adapter on an OS-assigned port and wait
/// for `app.bind().await` to surface the listening address before returning.
/// The caller is guaranteed the socket is live by the time it gets the port.
async fn start_rpc_server(module: impl toni::ModuleMetadata + 'static) -> u16 {
    use toni::toni_factory::ToniFactory;
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", 0))
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
    socket.connect(format!("127.0.0.1:{}", port)).await.unwrap();

    let mut frame = serde_json::json!({"pattern": pattern, "data": data});
    if let Some(id) = id {
        frame["id"] = serde_json::Value::String(id.to_string());
    }

    // Defensive retry on send errors (kernel queue pressure on the local
    // socket). The server is already bound by the time we get here — the
    // port channel guarantees readiness — so this isn't covering bind timing.
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

#[rpc_controller]
pub struct UdpRpcController {}
#[patterns]
impl UdpRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("udp.echo")]
    async fn echo(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }

    #[message_pattern("udp.panic")]
    async fn panic_handler(&self, _d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        panic!("intentional udp rpc panic");
    }

    #[event_pattern("udp.shipped")]
    async fn shipped(&self, _d: RpcData, _c: &RpcContext) -> Result<(), RpcError> {
        Ok(())
    }
}

#[module(controllers: [UdpRpcController])]
impl UdpRpcModule {}

#[tokio_localset_test::localset_test]
async fn udp_request_response_round_trips() {
    let port = start_rpc_server(UdpRpcModule).await;

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
    let port = start_rpc_server(UdpRpcModule).await;

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

/// A panicking RPC handler is caught by the dispatcher, surfaced as a
/// `PanicRecovered` framework event, and rendered through
/// `RpcError::to_data`. The reply is a canonical-envelope success
/// frame (not a wire-Err) and the server stays responsive on subsequent
/// datagrams.
///
/// Note: the test produces a "panicked at" line in stderr — that is the Rust
/// panic hook firing before catch_unwind catches the unwind. It is expected.
#[tokio_localset_test::localset_test]
async fn udp_panicking_handler_returns_error_frame() {
    let port = start_rpc_server(UdpRpcModule).await;

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
    let payload = &resp["response"];
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["kind"], "Internal");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("intentional udp rpc panic"),
        "panic message should surface in the envelope, got: {resp}",
    );

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
    let port = start_rpc_server(UdpRpcModule).await;

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

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(UdpRpcModule).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", 0))
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
        .send(
            "noop",
            RpcData::Json(serde_json::json!("hi")),
            Default::default(),
        )
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
        .send(
            "noop",
            RpcData::Json(serde_json::json!("hi")),
            Default::default(),
        )
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

    let port = start_rpc_server(UdpRpcModule).await;

    let transport = toni_udp::UdpClientTransport::new("127.0.0.1", port)
        .with_timeout(Duration::from_millis(500));

    let reply = transport
        .send(
            "udp.echo",
            RpcData::Json(serde_json::json!({"x": 1})),
            Default::default(),
        )
        .await
        .expect("UdpClientTransport.send should succeed");
    match reply {
        RpcData::Json(v) => assert_eq!(v, serde_json::json!({"x": 1})),
        other => panic!("expected json reply, got {other:?}"),
    }

    // Oversized payload: > 65 507 bytes should be rejected up-front.
    let huge = serde_json::Value::String("a".repeat(70_000));
    let err = transport
        .send("udp.echo", RpcData::Json(huge), Default::default())
        .await
        .expect_err("oversized payload should fail");
    match err {
        toni::RpcClientError::Transport(msg) => assert!(msg.contains("exceeds")),
        other => panic!("expected Transport error, got {other:?}"),
    }
}

#[rpc_controller]
pub struct SlowUdpController {}
#[patterns]
impl SlowUdpController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("udp.slow")]
    async fn slow(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(data)
    }
}

#[module(controllers: [SlowUdpController])]
impl SlowUdpModule {}

/// A datagram handler already running when shutdown fires must finish
/// during the drain window — its reply must arrive on the client socket.
#[tokio_localset_test::localset_test]
async fn udp_in_flight_request_completes_during_drain() {
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowUdpModule).await;
        app.use_rpc_adapter(toni_udp::UdpAdapter::new("127.0.0.1", 0))
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

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(format!("127.0.0.1:{}", port)).await.unwrap();
    let frame = serde_json::json!({"pattern":"udp.slow","data":"hi","id":"1"}).to_string();
    client.send(frame.as_bytes()).await.unwrap();

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

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowUdpModule).await;
        let adapter =
            toni_udp::UdpAdapter::new("127.0.0.1", 0).with_drain_timeout(Duration::from_millis(50));
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

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(format!("127.0.0.1:{}", port)).await.unwrap();
    let frame = serde_json::json!({"pattern":"udp.slow","data":"hi","id":"1"}).to_string();
    client.send(frame.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    shutdown.shutdown();

    // 50 ms drain budget + slack — must resolve well before the 300 ms handler.
    tokio::time::timeout(Duration::from_millis(500), shutdown.completed())
        .await
        .expect("shutdown must complete after drain timeout aborts the in-flight task");
}

/// With `with_max_inflight(1)` and a slow handler holding the only slot, a
/// concurrent datagram must be rejected immediately with an `"overloaded"`
/// frame. After the slow handler completes the slot is released and a
/// follow-up datagram succeeds.
#[tokio_localset_test::localset_test]
async fn udp_backpressure_rejects_excess_and_releases_after_completion() {
    use toni::toni_factory::ToniFactory;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(SlowUdpModule).await;
        let adapter = toni_udp::UdpAdapter::new("127.0.0.1", 0).with_max_inflight(1);
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

    // Client 1: occupy the only slot with a slow handler.
    let client1 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client1
        .connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let frame = serde_json::json!({"pattern":"udp.slow","data":"first","id":"1"}).to_string();
    client1.send(frame.as_bytes()).await.unwrap();
    // Give the server time to spawn the handler so the slot is genuinely held.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client 2: must be rejected with "overloaded" while the slot is full.
    let client2 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client2
        .connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let frame = serde_json::json!({"pattern":"udp.slow","data":"second","id":"2"}).to_string();
    client2.send(frame.as_bytes()).await.unwrap();

    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(Duration::from_millis(200), client2.recv(&mut buf))
        .await
        .expect("rejection should arrive immediately")
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(v["id"], "2");
    assert_eq!(v["err"]["status"], "overloaded");

    // Wait for client 1's reply — slot is freed when this handler finishes.
    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(Duration::from_secs(2), client1.recv(&mut buf))
        .await
        .expect("first handler should reply within 2s")
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(v["response"], "first");

    // Slot is free — a fresh datagram from client 2 succeeds.
    let frame = serde_json::json!({"pattern":"udp.slow","data":"third","id":"3"}).to_string();
    client2.send(frame.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(Duration::from_secs(2), client2.recv(&mut buf))
        .await
        .expect("third handler should reply after slot is freed")
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(v["response"], "third");
}

// ---- Metadata round-trip -----------------------------------------------------
//
// Metadata set on the client builder must ride the UDP frame's `metadata`
// field and surface in the handler's RpcContext.

#[rpc_controller]
pub struct UdpMetaController {}
#[patterns]
impl UdpMetaController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("meta.echo")]
    async fn meta_echo(&self, _d: RpcData, c: &RpcContext) -> Result<RpcData, RpcError> {
        let trace = c.get_metadata("trace").unwrap_or("none").to_string();
        Ok(RpcData::json(serde_json::json!({ "trace": trace })))
    }
}

#[module(controllers: [UdpMetaController])]
impl UdpMetaModule {}

#[tokio_localset_test::localset_test]
async fn udp_client_metadata_reaches_handler() {
    use std::time::Duration;
    use toni::RpcClient;

    let port = start_rpc_server(UdpMetaModule).await;
    let client = RpcClient::new(
        toni_udp::UdpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_millis(500)),
    );

    let resp = client
        .request("meta.echo")
        .metadata("trace", "abc123")
        .send(RpcData::json(serde_json::json!({})))
        .await
        .expect("metadata request should round-trip");
    assert_eq!(
        resp.as_json().and_then(|v| v["trace"].as_str()),
        Some("abc123"),
        "client metadata must reach the handler over UDP"
    );
}
