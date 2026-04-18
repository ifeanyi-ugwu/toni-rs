//! Verifies that a panic inside an RPC message handler returns an error
//! response to the caller instead of leaving the request hanging indefinitely,
//! and that the connection stays alive for subsequent messages.

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use serial_test::serial;
use toni::module;
use toni::rpc::{RpcContext, RpcData, RpcError};
use toni_macros::rpc_controller;

static RPC_PORT: AtomicU16 = AtomicU16::new(32000);

async fn start_rpc_server(module: toni::module_helpers::module_enum::ModuleDefinition) -> u16 {
    use toni::toni_factory::ToniFactory;
    let port = RPC_PORT.fetch_add(1, Ordering::SeqCst);
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        let _ = app.start().await;
    });
    tokio::task::spawn_local(async move { local.await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    port
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
    use tokio::net::TcpStream;

    let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(deadline, reader.read_line(&mut line))
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
    async fn panic_handler(
        &self,
        _d: RpcData,
        _c: RpcContext,
    ) -> Result<RpcData, RpcError> {
        panic!("intentional rpc panic");
    }

    #[message_pattern("rpc.safe")]
    async fn safe_handler(
        &self,
        _d: RpcData,
        _c: RpcContext,
    ) -> Result<RpcData, RpcError> {
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
#[serial]
#[tokio_localset_test::localset_test]
async fn rpc_handler_panic_returns_error_and_keeps_connection_alive() {
    let port = start_rpc_server(RpcPanicModule::module_definition()).await;

    // Panicking handler must return an error response within 500 ms,
    // not leave the caller hanging.
    let resp = tcp_rpc_timeout(port, "rpc.panic", serde_json::json!({}), Duration::from_millis(500)).await;
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
    let resp = tcp_rpc_timeout(port, "rpc.safe", serde_json::json!({}), Duration::from_millis(500)).await;
    assert_eq!(resp.unwrap()["response"], "safe-ok");
}
