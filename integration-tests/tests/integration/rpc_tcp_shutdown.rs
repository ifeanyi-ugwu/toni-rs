//! `app.shutdown()` must drive the TCP adapter's accept loop to exit;
//! otherwise `shutdown.completed().await` would hang forever. After
//! completion, `connect` to the listener fails (the socket is closed).

use std::time::Duration;

use toni::module;
use toni::rpc::{RpcContext, RpcData, RpcError};
use toni_macros::rpc_controller;

async fn pick_free_tcp_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
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

#[tokio_localset_test::localset_test]
async fn tcp_app_shutdown_stops_the_accept_loop() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toni::toni_factory::ToniFactory;

    let port = pick_free_tcp_port().await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(ShutdownTcpModule::module_definition()).await;
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.bind().await.unwrap();
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let shutdown = shutdown_rx.await.unwrap();

    // Server is responsive before shutdown.
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("server should accept before shutdown");
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
