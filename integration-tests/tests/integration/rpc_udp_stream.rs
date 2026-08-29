//! End-to-end coverage for streaming RPC replies over UDP (ADR-0032):
//!
//! - the wire grammar — one datagram per item frame, the end marker,
//!   mid-stream errors on both lanes
//! - a stream item too large for a datagram ends the stream loudly
//! - the producer stops on a cancel datagram and on a dropped client stream
//! - `RpcClient::stream` end to end, including against a single-reply handler
//! - `send()` against a streaming handler fails loudly

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use toni::context::{HandlerContext, RpcContext};
use toni::rpc::{RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};
use toni_macros::{controller, module, new, patterns};

async fn start_rpc_server(module: impl toni::ModuleMetadata + 'static) -> u16 {
    use toni::toni_factory::ToniFactory;
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let factory = ToniFactory::new();
        let mut app = factory.create_with(module).await.unwrap();
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

/// Send one request and collect the id-matched reply datagrams until an `end`
/// frame or the deadline.
async fn udp_stream_frames(
    port: u16,
    pattern: &str,
    data: serde_json::Value,
    deadline: Duration,
) -> Vec<serde_json::Value> {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(("127.0.0.1", port)).await.unwrap();

    let frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    socket.send(frame.as_bytes()).await.unwrap();

    let mut frames = Vec::new();
    let mut buf = vec![0u8; 65_507];
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let read = tokio::time::timeout_at(end, socket.recv(&mut buf)).await;
        let Ok(Ok(n)) = read else { break };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..n]) else {
            continue;
        };
        if v["id"] != "1" {
            continue;
        }
        let is_end = v["end"] == true;
        frames.push(v);
        if is_end {
            break;
        }
    }
    frames
}

/// Wait for a flag the server side flips, bounded.
async fn poll_flag(flag: &'static AtomicBool) -> bool {
    for _ in 0..40 {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

static CANCEL_DATAGRAM_SEEN: AtomicBool = AtomicBool::new(false);
static CLIENT_DROP_SEEN: AtomicBool = AtomicBool::new(false);

/// An unending tick stream whose producer records observing the execution's
/// cancellation token.
fn ticker(
    ctx: &RpcContext,
    saw_cancel: &'static AtomicBool,
) -> BoxStream<'static, Result<RpcData, RpcError>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<RpcData, RpcError>>(1);
    let token = ctx.cancellation().clone();
    tokio::spawn(async move {
        let mut n = 0u32;
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    saw_cancel.store(true, Ordering::SeqCst);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(30)) => {
                    n += 1;
                    if tx.send(Ok(RpcData::json(serde_json::json!(n)))).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx).boxed()
}

#[controller]
pub struct UdpStreamController {}
#[patterns]
impl UdpStreamController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("count.stream")]
    async fn count(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter((1..=3).map(|n| Ok(RpcData::json(serde_json::json!(n)))))
                .boxed(),
        ))
    }

    #[message_pattern("bytes.stream")]
    async fn bytes(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(vec![Ok(RpcData::binary(vec![0, 159, 146, 150]))]).boxed(),
        ))
    }

    #[message_pattern("interr.stream")]
    async fn internal_err(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(vec![Err(RpcError::Internal("cursor died".into()))]).boxed(),
        ))
    }

    #[message_pattern("oversize.stream")]
    async fn oversize(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(vec![
                Ok(RpcData::text("x".repeat(70_000))),
                Ok(RpcData::json(serde_json::json!(2))),
            ])
            .boxed(),
        ))
    }

    #[message_pattern("probe.cancel")]
    async fn probe_cancel(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(ticker(ctx, &CANCEL_DATAGRAM_SEEN)))
    }

    #[message_pattern("probe.client_drop")]
    async fn probe_client_drop(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(ticker(ctx, &CLIENT_DROP_SEEN)))
    }

    #[message_pattern("single.echo")]
    async fn single_echo(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("one-shot")))
    }
}

#[module(controllers: [UdpStreamController])]
impl UdpStreamModule {}

#[tokio_localset_test::localset_test]
async fn stream_datagrams_arrive_in_order_then_the_end_marker() {
    let port = start_rpc_server(UdpStreamModule).await;
    let frames = udp_stream_frames(
        port,
        "count.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;

    assert_eq!(frames.len(), 4, "3 items + end, got {frames:?}");
    for (i, frame) in frames[..3].iter().enumerate() {
        assert_eq!(frame["stream"], (i as u64) + 1);
    }
    assert_eq!(frames[3]["end"], true);
    assert!(frames[3].get("err").is_none());
}

#[tokio_localset_test::localset_test]
async fn a_binary_item_travels_base64_and_decodes_back() {
    let port = start_rpc_server(UdpStreamModule).await;
    let client = toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", port));
    let mut stream = client
        .stream("bytes.stream", RpcData::json(serde_json::json!(null)))
        .await
        .unwrap();
    match stream.next().await {
        Some(Ok(RpcData::Binary(b))) => assert_eq!(b, vec![0, 159, 146, 150]),
        other => panic!("expected a Binary item, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
}

#[tokio_localset_test::localset_test]
async fn a_framework_error_mid_stream_is_an_error_end() {
    let port = start_rpc_server(UdpStreamModule).await;
    let frames = udp_stream_frames(
        port,
        "interr.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(frames.len(), 1, "got {frames:?}");
    assert_eq!(frames[0]["end"], true);
    assert_eq!(frames[0]["err"]["status"], "error");
}

#[tokio_localset_test::localset_test]
async fn an_oversize_item_ends_the_stream_loudly() {
    let port = start_rpc_server(UdpStreamModule).await;
    let frames = udp_stream_frames(
        port,
        "oversize.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(frames.len(), 1, "got {} frames", frames.len());
    assert_eq!(frames[0]["end"], true);
    let msg = frames[0]["err"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("exceeds"), "got: {msg}");
}

#[tokio_localset_test::localset_test]
async fn a_cancel_datagram_stops_the_producer() {
    let port = start_rpc_server(UdpStreamModule).await;
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(("127.0.0.1", port)).await.unwrap();

    let frame = serde_json::json!({"pattern": "probe.cancel", "data": null, "id": "1"}).to_string();
    socket.send(frame.as_bytes()).await.unwrap();

    // One item proves the stream is live before the cancel.
    let mut buf = vec![0u8; 65_507];
    tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut buf))
        .await
        .expect("first item")
        .unwrap();

    let cancel = serde_json::json!({"id": "1", "cancel": true}).to_string();
    socket.send(cancel.as_bytes()).await.unwrap();

    assert!(
        poll_flag(&CANCEL_DATAGRAM_SEEN).await,
        "producer never observed the cancellation token"
    );
}

#[tokio_localset_test::localset_test]
async fn an_early_client_drop_sends_the_cancel_notice() {
    let port = start_rpc_server(UdpStreamModule).await;
    let client = toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", port));
    let mut stream = client
        .stream("probe.client_drop", RpcData::json(serde_json::json!(null)))
        .await
        .unwrap();
    assert!(stream.next().await.is_some(), "first item");
    drop(stream);

    assert!(
        poll_flag(&CLIENT_DROP_SEEN).await,
        "producer never observed the cancellation token"
    );
}

#[tokio_localset_test::localset_test]
async fn a_stream_call_to_a_single_handler_is_one_item_then_the_end() {
    let port = start_rpc_server(UdpStreamModule).await;
    let client = toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", port));
    let mut stream = client
        .stream("single.echo", RpcData::json(serde_json::json!(null)))
        .await
        .unwrap();
    match stream.next().await {
        Some(Ok(data)) => assert_eq!(data.parse::<String>().unwrap(), "one-shot"),
        other => panic!("expected one item, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
}

#[tokio_localset_test::localset_test]
async fn a_send_to_a_streaming_handler_fails_loudly() {
    let port = start_rpc_server(UdpStreamModule).await;
    let client = toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", port));
    match client
        .send("count.stream", RpcData::json(serde_json::json!(null)))
        .await
    {
        Err(toni::RpcClientError::Transport(msg)) => {
            assert!(msg.contains("use stream()"), "got: {msg}")
        }
        other => panic!("expected a loud transport error, got {other:?}"),
    }
}
