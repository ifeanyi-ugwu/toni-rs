// Streaming RPC replies over TCP (ADR-0032).
//
// This example demonstrates:
// 1. A #[message_pattern] handler answering with a stream — declare
//    `-> RpcHandlerResult` and construct RpcHandlerOutput::Stream
// 2. Consuming the reply with RpcClient::stream — items until the end marker
// 3. Cancellation by dropping the reply stream — the server aborts the call
//    and the producer hears `ctx.cancellation()`
//
// Wire: one newline-delimited JSON frame per item ({"id","stream":…}),
// closed by {"id","end":true}; dropping the client stream sends
// {"id","cancel":true}.
//
// Run: cargo run --example rpc_streaming

use std::time::Duration;

use futures::StreamExt;
use toni::context::{HandlerContext, RpcContext};
use toni::rpc::{RpcHandlerOutput, RpcHandlerResult};
use toni::{RpcClient, RpcData, RpcError, ToniFactory};
use toni_macros::{controller, module, new, patterns};

#[controller]
pub struct FeedController {}
#[patterns]
impl FeedController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    // A bounded stream: three items, then the end marker.
    #[message_pattern("feed.count")]
    async fn count(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Stream(
            futures::stream::iter((1..=3).map(|n| Ok(RpcData::json(serde_json::json!(n))))).boxed(),
        ))
    }

    // An unending stream: ticks until the caller goes away. The producer
    // outlives the handler, so it watches the execution's cancellation token —
    // that is how it learns the caller dropped the stream.
    #[message_pattern("feed.ticks")]
    async fn ticks(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<RpcData, RpcError>>(1);
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        println!("[server] caller left — producer stops");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        n += 1;
                        if tx.send(Ok(RpcData::json(serde_json::json!({ "tick": n })))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(RpcHandlerOutput::Stream(
            tokio_stream::wrappers::ReceiverStream::new(rx).boxed(),
        ))
    }
}

#[module(controllers: [FeedController])]
struct FeedModule;

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async {
        // Server on an OS-assigned port; the bound address comes back from bind().
        let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
        tokio::task::spawn_local(async move {
            let mut app = ToniFactory::new().create_with(FeedModule).await.unwrap();
            app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
                .unwrap();
            let bound = app.bind().await.unwrap();
            let _ = port_tx.send(bound.rpc.unwrap().port());
            app.run().await;
        });
        let port = port_rx.await?;
        let client = RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", port));

        // A bounded stream ends on its own.
        let items: Vec<i64> = client
            .stream("feed.count", RpcData::json(serde_json::json!(null)))
            .await?
            .map(|item| item.unwrap().parse::<i64>().unwrap())
            .collect()
            .await;
        println!("[client] feed.count -> {items:?}");
        assert_eq!(items, vec![1, 2, 3]);

        // An unending stream ends when the caller drops it: take two ticks,
        // drop, and the server's producer stops.
        let mut ticks = client
            .stream("feed.ticks", RpcData::json(serde_json::json!(null)))
            .await?;
        for _ in 0..2 {
            let tick = ticks.next().await.expect("tick")?;
            println!("[client] feed.ticks -> {tick:?}");
        }
        drop(ticks);

        // Give the cancel frame a beat to reach the server before exiting, so
        // the producer's stop line prints.
        tokio::time::sleep(Duration::from_millis(300)).await;
        anyhow::Ok(())
    }))?;
    Ok(())
}
