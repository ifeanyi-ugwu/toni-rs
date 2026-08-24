//! Conformance suite for the RPC and gRPC adapters serving on a socket the
//! caller acquired: `TcpAdapter::from_listener`, `UdpAdapter::from_socket`,
//! `GrpcAdapter::from_listener`.
//!
//! The proof that the adapter adopted the socket rather than binding its own
//! is address identity: the test records the socket's address before handing
//! it over, then requires `BoundAdapters` to report that exact address and the
//! server to answer on it. A fresh bind on port 0 would land somewhere else.
//!
//! The five broker transports (NATS, Redis, RabbitMQ, MQTT, Kafka) have no
//! listening socket to adopt, so they are absent by construction.

use std::net::SocketAddr;
use std::time::Duration;

use toni::context::RpcContext;
use toni::module;
use toni::rpc::{RpcData, RpcError};
use toni::toni_factory::ToniFactory;
use toni_macros::{message_pattern, new, patterns, rpc_controller};

#[rpc_controller]
pub struct AdoptionController {}

#[patterns]
impl AdoptionController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("adoption.echo")]
    async fn echo(&self, data: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [AdoptionController])]
impl AdoptionModule {}

/// Start an app whose RPC adapter was built from a caller-owned socket, and
/// return the address `bind()` reports for it.
async fn start_rpc_on(adapter: impl toni::RpcAdapter) -> SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(AdoptionModule).await.unwrap();
        app.use_rpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.rpc.expect("RPC adapter must report its address"));
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    addr_rx.await.expect("RPC server failed to start")
}

#[tokio_localset_test::localset_test]
async fn tcp_serves_on_caller_supplied_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let expected = listener.local_addr().unwrap();

    let reported = start_rpc_on(toni_tcp::TcpAdapter::from_listener(listener)).await;
    assert_eq!(
        reported, expected,
        "adapter reported a different address than the listener it was given"
    );

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stream = tokio::net::TcpStream::connect(expected).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame =
        serde_json::json!({"pattern": "adoption.echo", "data": {"hello": "tcp"}, "id": "1"})
            .to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("adopted listener must answer within 2s")
        .unwrap();

    let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(resp["response"], serde_json::json!({"hello": "tcp"}));
}

#[tokio_localset_test::localset_test]
async fn udp_serves_on_caller_supplied_socket() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let expected = socket.local_addr().unwrap();

    let reported = start_rpc_on(toni_udp::UdpAdapter::from_socket(socket)).await;
    assert_eq!(
        reported, expected,
        "adapter reported a different address than the socket it was given"
    );

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(expected).await.unwrap();
    let frame =
        serde_json::json!({"pattern": "adoption.echo", "data": {"hello": "udp"}, "id": "1"})
            .to_string();
    client.send(frame.as_bytes()).await.unwrap();

    let mut buf = vec![0u8; 65_507];
    let n = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut buf))
        .await
        .expect("adopted socket must answer within 2s")
        .unwrap();

    let resp: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(resp["response"], serde_json::json!({"hello": "udp"}));
}

#[tokio_localset_test::localset_test]
async fn grpc_serves_on_caller_supplied_listener() {
    use tonic_health::pb::health_check_response::ServingStatus as PbServingStatus;
    use tonic_health::pb::health_client::HealthClient;
    use tonic_health::pb::HealthCheckRequest;
    use tonic_health::ServingStatus;

    #[toni::module()]
    struct EmptyModule;

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("adoption.service", ServingStatus::Serving)
        .await;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let expected = listener.local_addr().unwrap();
    let adapter = toni_grpc::GrpcAdapter::from_listener(listener).add_service(health_service);

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(EmptyModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.grpc.expect("gRPC adapter must report its address"));
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let reported = addr_rx.await.expect("gRPC server failed to start");
    assert_eq!(
        reported, expected,
        "adapter reported a different address than the listener it was given"
    );

    let mut client = tonic::transport::Endpoint::from_shared(format!("http://{expected}"))
        .unwrap()
        .connect()
        .await
        .map(HealthClient::new)
        .expect("adopted listener must accept a gRPC connection");

    let resp = tokio::time::timeout(
        Duration::from_secs(2),
        client.check(HealthCheckRequest {
            service: "adoption.service".to_string(),
        }),
    )
    .await
    .expect("health check must reply within 2s")
    .expect("health check must succeed");

    assert_eq!(resp.into_inner().status, PbServingStatus::Serving as i32);
}
