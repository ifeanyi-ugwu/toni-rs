use std::sync::Arc;
use testcontainers::{runners::AsyncRunner, ContainerAsync};
use testcontainers_modules::redis::{Redis, REDIS_PORT};
use tokio::sync::mpsc;
use toni::{
    module_helpers::module_enum::ModuleDefinition,
    toni_factory::ToniFactory,
    websocket::{WsMessage, WsSink},
};
use toni_axum::TokioSender;
use toni_redis_broadcast::{RedisBroadcastModule, RedisBroadcastService};

fn make_client() -> (Arc<dyn WsSink>, mpsc::Receiver<WsMessage>) {
    let (tx, rx) = mpsc::channel(16);
    (Arc::new(TokioSender::new(tx)) as Arc<dyn WsSink>, rx)
}

async fn boot(url: &str) -> (toni::ToniApplicationContext, RedisBroadcastService) {
    let app = ToniFactory::create_application_context(ModuleDefinition::DefaultModule(
        Box::new(RedisBroadcastModule::for_root(url)),
    ))
    .await;
    let rbs = app
        .get::<RedisBroadcastService>()
        .await
        .expect("RedisBroadcastService not found in DI — likely a token mismatch");
    (app, rbs)
}

async fn start_redis() -> (ContainerAsync<Redis>, String) {
    let container = Redis::default().start().await.unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(REDIS_PORT).await.unwrap();
    // The container-side "Ready to accept connections" fires before the host-side
    // port-forwarding tunnel is live (common with Rancher Desktop / Lima VMs).
    // Retry until the port is actually reachable from the host.
    wait_for_tcp(&host.to_string(), port).await;
    let url = format!("redis://{host}:{port}/");
    (container, url)
}

async fn wait_for_tcp(host: &str, port: u16) {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect((host, port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("Redis at {host}:{port} not reachable after 10s");
}

/// Wait up to 2 s for a message to arrive.
async fn recv(rx: &mut mpsc::Receiver<WsMessage>) -> WsMessage {
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for message from Redis subscriber loop")
        .expect("channel closed before message arrived")
}

// ---------------------------------------------------------------------------

/// `to_all()` publishes to Redis; the subscriber loop delivers it locally.
#[tokio::test]
#[ignore = "requires Docker"]
async fn to_all_round_trips_through_redis() {
    let (_container, url) = start_redis().await;
    let (_app, rbs) = boot(&url).await;

    let (sink1, mut rx1) = make_client();
    let (sink2, mut rx2) = make_client();
    rbs.connect("c1".to_string(), sink1, None);
    rbs.connect("c2".to_string(), sink2, None);

    rbs.to_all()
        .send(WsMessage::text("hello everyone"))
        .await
        .unwrap();

    assert_eq!(recv(&mut rx1).await.as_text(), Some("hello everyone"));
    assert_eq!(recv(&mut rx2).await.as_text(), Some("hello everyone"));
}

/// `to_room()` only delivers to clients that joined the room.
#[tokio::test]
#[ignore = "requires Docker"]
async fn to_room_delivers_only_to_members() {
    let (_container, url) = start_redis().await;
    let (_app, rbs) = boot(&url).await;

    let (member_sink, mut member_rx) = make_client();
    let (outsider_sink, mut outsider_rx) = make_client();
    rbs.connect("member".to_string(), member_sink, None);
    rbs.connect("outsider".to_string(), outsider_sink, None);

    rbs.join_room("member", "vip").unwrap();

    rbs.to_room("vip")
        .send(WsMessage::text("vip only"))
        .await
        .unwrap();

    assert_eq!(recv(&mut member_rx).await.as_text(), Some("vip only"));
    assert!(
        outsider_rx.try_recv().is_err(),
        "outsider should not receive room message"
    );
}

/// `to_client()` delivers only to the addressed client.
#[tokio::test]
#[ignore = "requires Docker"]
async fn to_client_delivers_only_to_target() {
    let (_container, url) = start_redis().await;
    let (_app, rbs) = boot(&url).await;

    let (target_sink, mut target_rx) = make_client();
    let (bystander_sink, mut bystander_rx) = make_client();
    rbs.connect("target".to_string(), target_sink, None);
    rbs.connect("bystander".to_string(), bystander_sink, None);

    rbs.to_client("target")
        .send(WsMessage::text("private"))
        .await
        .unwrap();

    assert_eq!(recv(&mut target_rx).await.as_text(), Some("private"));
    assert!(
        bystander_rx.try_recv().is_err(),
        "bystander should not receive private message"
    );
}

/// Two independent instances sharing one Redis — a message published by instance 2
/// is delivered to a client connected to instance 1.  This is the actual cross-process
/// guarantee: separate `BroadcastService` maps, nothing shared in memory.
#[tokio::test]
#[ignore = "requires Docker"]
async fn cross_process_delivery() {
    let (_container, url) = start_redis().await;

    let (_app1, rbs1) = boot(&url).await;
    let (_app2, rbs2) = boot(&url).await;

    // Client lives on instance 1.
    let (sink, mut rx) = make_client();
    rbs1.connect("client-on-1".to_string(), sink, None);

    // Instance 2 publishes — it has no idea about client-on-1.
    rbs2.to_all()
        .send(WsMessage::text("cross-process"))
        .await
        .unwrap();

    assert_eq!(recv(&mut rx).await.as_text(), Some("cross-process"));
}

/// `send_event()` wraps the payload in `{"event": ..., "data": ...}`.
#[tokio::test]
#[ignore = "requires Docker"]
async fn send_event_formats_payload_correctly() {
    let (_container, url) = start_redis().await;
    let (_app, rbs) = boot(&url).await;

    let (sink, mut rx) = make_client();
    rbs.connect("c".to_string(), sink, None);

    rbs.to_all()
        .send_event("user.joined", r#"{"name":"Alice"}"#)
        .await
        .unwrap();

    let msg = recv(&mut rx).await;
    let text = msg.as_text().expect("expected text message");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["event"], "user.joined");
    assert_eq!(v["data"], r#"{"name":"Alice"}"#);
}
