//! End-to-end coverage for streaming RPC replies over TCP (ADR-0032):
//!
//! - the wire grammar — item frames, the end marker, mid-stream errors on
//!   both lanes
//! - the execution's bag stays readable across the drain
//! - the producer stops on every abandonment path: a cancel frame, a dropped
//!   connection, a dropped client stream, and a cancel before the first item
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

/// Send one request and collect the id-matched reply frames until an `end`
/// frame or the deadline.
async fn tcp_stream_frames(
    port: u16,
    pattern: &str,
    data: serde_json::Value,
    deadline: Duration,
) -> Vec<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut frames = Vec::new();
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let mut line = String::new();
        let read = tokio::time::timeout_at(end, reader.read_line(&mut line)).await;
        let Ok(Ok(n)) = read else { break };
        if n == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
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

static CANCEL_FRAME_SEEN: AtomicBool = AtomicBool::new(false);
static DISCONNECT_SEEN: AtomicBool = AtomicBool::new(false);
static CLIENT_DROP_SEEN: AtomicBool = AtomicBool::new(false);
static HANDLER_DROPPED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, toni::Error)]
#[error_kind(Conflict)]
struct Spilled;

impl std::fmt::Display for Spilled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the cursor spilled")
    }
}

impl std::error::Error for Spilled {}

#[derive(Clone)]
struct Tag(&'static str);

/// An unending tick stream whose producer records observing the execution's
/// cancellation token — the flag is how a test sees the drop side effect.
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
pub struct StreamController {}
#[patterns]
impl StreamController {
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

    #[message_pattern("apperr.stream")]
    async fn app_err(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(vec![
                Ok(RpcData::json(serde_json::json!(1))),
                Err(Spilled.into()),
            ])
            .boxed(),
        ))
    }

    #[message_pattern("interr.stream")]
    async fn internal_err(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(vec![Err(RpcError::Internal("cursor died".into()))]).boxed(),
        ))
    }

    #[message_pattern("bag.stream")]
    async fn bag(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        ctx.extensions().insert(Tag("alive"));
        let ctx = ctx.clone();
        Ok(RpcHandlerOutput::Stream(
            futures_util::stream::iter(0..3)
                .map(move |n| {
                    let tag = ctx.extensions().get::<Tag>().map(|t| t.0).unwrap_or("gone");
                    Ok(RpcData::json(serde_json::json!(format!("{n}:{tag}"))))
                })
                .boxed(),
        ))
    }

    #[message_pattern("probe.cancel")]
    async fn probe_cancel(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(ticker(ctx, &CANCEL_FRAME_SEEN)))
    }

    #[message_pattern("probe.disconnect")]
    async fn probe_disconnect(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(ticker(ctx, &DISCONNECT_SEEN)))
    }

    #[message_pattern("probe.client_drop")]
    async fn probe_client_drop(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(ticker(ctx, &CLIENT_DROP_SEEN)))
    }

    #[message_pattern("probe.slow")]
    async fn probe_slow(&self, _d: RpcData) -> RpcHandlerResult {
        struct Sentinel;
        impl Drop for Sentinel {
            fn drop(&mut self) {
                HANDLER_DROPPED.store(true, Ordering::SeqCst);
            }
        }
        let _sentinel = Sentinel;
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(RpcHandlerOutput::Single(RpcData::text("too late")))
    }

    #[message_pattern("single.echo")]
    async fn single_echo(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("one-shot")))
    }
}

#[module(controllers: [StreamController])]
impl StreamModule {}

#[tokio_localset_test::localset_test]
async fn stream_frames_arrive_in_order_then_the_end_marker() {
    let port = start_rpc_server(StreamModule).await;
    let frames = tcp_stream_frames(
        port,
        "count.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;

    assert_eq!(frames.len(), 4, "3 items + end, got {frames:?}");
    for (i, frame) in frames[..3].iter().enumerate() {
        assert_eq!(frame["stream"], (i as u64) + 1);
        assert!(
            frame.get("response").is_none(),
            "item frames never use the response key"
        );
    }
    assert_eq!(frames[3]["end"], true);
    assert!(frames[3].get("err").is_none());
}

#[tokio_localset_test::localset_test]
async fn a_binary_item_travels_base64_and_decodes_back() {
    let port = start_rpc_server(StreamModule).await;
    let frames = tcp_stream_frames(
        port,
        "bytes.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;
    assert!(frames[0].get("stream_b64").is_some(), "got {frames:?}");

    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
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
async fn an_app_error_mid_stream_is_a_final_envelope_item_then_a_clean_end() {
    let port = start_rpc_server(StreamModule).await;
    let frames = tcp_stream_frames(
        port,
        "apperr.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;

    assert_eq!(frames.len(), 3, "item, envelope item, end — got {frames:?}");
    assert_eq!(frames[0]["stream"], 1);
    assert_eq!(frames[1]["stream"]["status"], "error");
    assert_eq!(frames[1]["stream"]["kind"], "Conflict");
    assert_eq!(frames[2]["end"], true);
    assert!(frames[2].get("err").is_none());
}

#[tokio_localset_test::localset_test]
async fn a_framework_error_mid_stream_is_an_error_end() {
    let port = start_rpc_server(StreamModule).await;
    let frames = tcp_stream_frames(
        port,
        "interr.stream",
        serde_json::json!(null),
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(frames.len(), 1, "got {frames:?}");
    assert_eq!(frames[0]["end"], true);
    assert_eq!(frames[0]["err"]["status"], "error");

    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
    let mut stream = client
        .stream("interr.stream", RpcData::json(serde_json::json!(null)))
        .await
        .unwrap();
    match stream.next().await {
        Some(Err(toni::RpcClientError::Remote { status, .. })) => assert_eq!(status, "error"),
        other => panic!("expected a Remote error item, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
}

#[tokio_localset_test::localset_test]
async fn the_bag_stays_readable_across_the_drain() {
    let port = start_rpc_server(StreamModule).await;
    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
    let stream = client
        .stream("bag.stream", RpcData::json(serde_json::json!(null)))
        .await
        .unwrap();
    let items: Vec<String> = stream
        .map(|item| item.unwrap().parse::<String>().unwrap())
        .collect()
        .await;
    assert_eq!(items, vec!["0:alive", "1:alive", "2:alive"]);
}

#[tokio_localset_test::localset_test]
async fn a_cancel_frame_stops_the_producer() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let port = start_rpc_server(StreamModule).await;
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame =
        serde_json::json!({"pattern": "probe.cancel", "data": null, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    // One item proves the stream is live before the cancel.
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("first item")
        .unwrap();

    let mut cancel = serde_json::json!({"id": "1", "cancel": true}).to_string();
    cancel.push('\n');
    writer.write_all(cancel.as_bytes()).await.unwrap();

    assert!(
        poll_flag(&CANCEL_FRAME_SEEN).await,
        "producer never observed the cancellation token"
    );
}

#[tokio_localset_test::localset_test]
async fn a_dropped_connection_stops_the_producer() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let port = start_rpc_server(StreamModule).await;
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame =
        serde_json::json!({"pattern": "probe.disconnect", "data": null, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("first item")
        .unwrap();

    drop(reader);
    drop(writer);

    assert!(
        poll_flag(&DISCONNECT_SEEN).await,
        "producer never observed the cancellation token"
    );
}

#[tokio_localset_test::localset_test]
async fn a_cancel_before_the_first_item_drops_the_handler_future() {
    use tokio::io::AsyncWriteExt;

    let port = start_rpc_server(StreamModule).await;
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();

    let mut frame =
        serde_json::json!({"pattern": "probe.slow", "data": null, "id": "1"}).to_string();
    frame.push('\n');
    stream.write_all(frame.as_bytes()).await.unwrap();

    // Give the handler its first poll — a cancel in the same packet aborts
    // the task before its body runs, which the sentinel cannot observe.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut cancel = serde_json::json!({"id": "1", "cancel": true}).to_string();
    cancel.push('\n');
    stream.write_all(cancel.as_bytes()).await.unwrap();

    // The handler sleeps 10 s; only an aborted future explains the sentinel
    // dropping within the poll window.
    assert!(
        poll_flag(&HANDLER_DROPPED).await,
        "handler future was not dropped"
    );
}

#[tokio_localset_test::localset_test]
async fn an_early_client_drop_sends_the_cancel_notice() {
    let port = start_rpc_server(StreamModule).await;
    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
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
    let port = start_rpc_server(StreamModule).await;
    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
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
    let port = start_rpc_server(StreamModule).await;
    let client = toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));
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
