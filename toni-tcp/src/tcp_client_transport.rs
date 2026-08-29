use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::SinkExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first frame carrying its id.
    Single(oneshot::Sender<Result<RpcData, RpcClientError>>),
    /// An `open_stream()` call: fed frame by frame until the end marker,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<Result<RpcData, RpcClientError>>),
}

struct Inner {
    writer: Mutex<tokio::net::tcp::OwnedWriteHalf>,
    // Correlation id → channel waiting for the server's reply.
    pending: Mutex<HashMap<String, PendingSlot>>,
    // Cancel notices for abandoned streaming calls; drained by cancel_loop,
    // which removes the pending entry and writes the in-band cancel frame.
    cancel_tx: mpsc::UnboundedSender<String>,
}

/// TCP transport for [`RpcClient`].
///
/// Maintains a persistent TCP connection to the remote service.
/// Request-response uses a monotonic correlation id; the background reader
/// loop matches each incoming `{"id":..., "response":...}` or `{"id":...,
/// "err":...}` frame to the waiting caller and delivers it via an in-memory
/// channel.
///
/// Fire-and-forget (`emit`) sends a frame with no `id` field and returns as
/// soon as the write completes.
///
/// A streaming call (`open_stream`, ADR-0032) sends the same request frame;
/// the reader loop feeds `{"id":…,"stream":…}` frames to the caller's
/// [`RpcReplyStream`] until `{"id":…,"end":true}`. The configured timeout
/// bounds the gap to the next frame, the first included. Dropping the reply
/// stream before its end sends `{"id":…,"cancel":true}`, aborting the call on
/// the server.
///
/// The transport rebinds lazily on the next call after a fatal error
/// (reader-loop exit, write error). Pending requests at the moment of
/// failure surface as `RpcClientError::Transport("connection closed")`.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "ORDERS_CLIENT",
///     toni::RpcClient::new(toni_tcp::TcpClientTransport::new("127.0.0.1", 4000))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
// Slot type shared between the public transport and the background reader
// loop. The reader clears the slot on exit so the next caller rebuilds the
// connection — this is the lazy-reconnect path.
type Slot = Arc<Mutex<Option<Arc<Inner>>>>;

pub struct TcpClientTransport {
    host: String,
    port: u16,
    timeout: Duration,
    slot: Slot,
}

impl TcpClientTransport {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            timeout: Duration::from_secs(5),
            slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the request-response timeout (default: 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn get_or_connect(&self) -> Result<Arc<Inner>, RpcClientError> {
        let mut guard = self.slot.lock().await;
        if let Some(inner) = guard.as_ref() {
            return Ok(inner.clone());
        }

        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let (reader, writer) = stream.into_split();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            cancel_tx,
        });

        tokio::spawn(reader_loop(reader, inner.clone(), self.slot.clone()));
        // Weak: the loop must not keep the connection alive once the slot is
        // cleared and every caller is gone — `Inner` holds the sender, so a
        // strong reference here would be a cycle.
        tokio::spawn(cancel_loop(cancel_rx, Arc::downgrade(&inner)));

        tracing::info!(addr, "TcpClientTransport connected");
        *guard = Some(inner.clone());
        Ok(inner)
    }

    /// Drop the cached connection so the next call rebuilds. Idempotent.
    async fn invalidate(&self) {
        self.slot.lock().await.take();
    }
}

async fn reader_loop(reader: tokio::net::tcp::OwnedReadHalf, inner: Arc<Inner>, slot: Slot) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let msg: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let Some(id) = msg["id"].as_str() else {
                    continue;
                };

                let mut pending = inner.pending.lock().await;
                match pending.get(id) {
                    None => {}
                    Some(PendingSlot::Single(_)) => {
                        let Some(PendingSlot::Single(tx)) = pending.remove(id) else {
                            unreachable!("slot variant checked above");
                        };
                        let result = match wire::parse_reply_frame(trimmed.as_bytes()) {
                            ReplyFrame::Single(result) => result,
                            ReplyFrame::Item(_) | ReplyFrame::End | ReplyFrame::EndErr { .. } => {
                                Err(RpcClientError::Transport(
                                    "streaming reply to a single-reply call — use stream()"
                                        .to_string(),
                                ))
                            }
                        };
                        let _ = tx.send(result);
                    }
                    Some(PendingSlot::Stream(tx)) => {
                        match wire::parse_reply_frame(trimmed.as_bytes()) {
                            ReplyFrame::Item(data) => {
                                let _ = tx.send(Ok(data));
                            }
                            // A single-reply answer to a stream call: one item,
                            // then the end.
                            ReplyFrame::Single(result) => {
                                let _ = tx.send(result);
                                pending.remove(id);
                            }
                            ReplyFrame::End => {
                                pending.remove(id);
                            }
                            ReplyFrame::EndErr { message, status } => {
                                let _ = tx.send(Err(RpcClientError::Remote { message, status }));
                                pending.remove(id);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "TcpClientTransport read error");
                break;
            }
        }
    }

    // Clear the cached connection so the next caller rebuilds.
    slot.lock().await.take();

    // Drain all pending requests so callers don't hang indefinitely.
    let mut pending = inner.pending.lock().await;
    for (_, slot) in pending.drain() {
        let closed = || RpcClientError::Transport("connection closed".to_string());
        match slot {
            PendingSlot::Single(tx) => {
                let _ = tx.send(Err(closed()));
            }
            PendingSlot::Stream(tx) => {
                let _ = tx.send(Err(closed()));
            }
        }
    }

    tracing::debug!("TcpClientTransport connection closed");
}

/// Feed one streaming call's frames from the reader loop into its
/// [`RpcReplyStream`], enforcing the per-frame gap deadline. On expiry the
/// caller sees an `Err(Timeout)` item and the server gets the cancel notice.
async fn forward_stream(
    mut raw_rx: mpsc::UnboundedReceiver<Result<RpcData, RpcClientError>>,
    mut sink: ReplySink,
    gap: Duration,
    id: String,
    inner: Arc<Inner>,
) {
    loop {
        match tokio::time::timeout(gap, raw_rx.recv()).await {
            Ok(Some(item)) => {
                if sink.send(item).await.is_err() {
                    // The caller dropped the stream; its drop already sent
                    // the cancel notice.
                    break;
                }
            }
            // End marker, connection loss, or cancel — the entry is gone and
            // the sender with it.
            Ok(None) => break,
            Err(_) => {
                let _ = sink.send(Err(RpcClientError::Timeout)).await;
                inner.pending.lock().await.remove(&id);
                let _ = inner.cancel_tx.send(id.clone());
                break;
            }
        }
    }
}

/// Retire abandoned streaming calls: drop the pending entry and tell the
/// server with an in-band `{"id", "cancel": true}` frame.
async fn cancel_loop(mut rx: mpsc::UnboundedReceiver<String>, weak: std::sync::Weak<Inner>) {
    while let Some(id) = rx.recv().await {
        let Some(inner) = weak.upgrade() else { break };
        inner.pending.lock().await.remove(&id);
        let mut line = serde_json::json!({ "id": id, "cancel": true }).to_string();
        line.push('\n');
        let mut writer = inner.writer.lock().await;
        if let Err(e) = writer.write_all(line.as_bytes()).await {
            tracing::debug!(error = %e, "TcpClientTransport cancel write failed");
        }
    }
}

fn data_to_json(data: RpcData) -> serde_json::Value {
    match data {
        RpcData::Json(v) => v,
        RpcData::Text(s) => serde_json::Value::String(s),
        // TCP wire format is JSON; binary payloads are not supported.
        RpcData::Binary(_) => serde_json::Value::Null,
    }
}

#[async_trait]
impl RpcClientTransport for TcpClientTransport {
    async fn connect(&self) -> Result<(), RpcClientError> {
        self.get_or_connect().await?;
        Ok(())
    }

    async fn send(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcData, RpcClientError> {
        let inner = self.get_or_connect().await?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();

        inner
            .pending
            .lock()
            .await
            .insert(id.clone(), PendingSlot::Single(tx));

        let mut msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
            "id": id,
        });
        if !metadata.is_empty() {
            msg["metadata"] = serde_json::json!(metadata);
        }
        let mut frame = msg.to_string();
        frame.push('\n');

        if let Err(e) = inner.writer.lock().await.write_all(frame.as_bytes()).await {
            inner.pending.lock().await.remove(&id);
            // Fatal-looking — drop the cached connection so the next call
            // rebuilds rather than retrying on a half-broken handle.
            self.invalidate().await;
            return Err(RpcClientError::Transport(e.to_string()));
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcClientError::Transport("connection closed".to_string())),
            Err(_) => {
                inner.pending.lock().await.remove(&id);
                Err(RpcClientError::Timeout)
            }
        }
    }

    async fn open_stream(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcReplyStream, RpcClientError> {
        let inner = self.get_or_connect().await?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();

        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        inner
            .pending
            .lock()
            .await
            .insert(id.clone(), PendingSlot::Stream(raw_tx));

        let cancel_tx = inner.cancel_tx.clone();
        let cancel_id = id.clone();
        let (sink, stream) = RpcReplyStream::channel(32, move || {
            let _ = cancel_tx.send(cancel_id);
        });
        tokio::spawn(forward_stream(
            raw_rx,
            sink,
            self.timeout,
            id.clone(),
            inner.clone(),
        ));

        // The request frame is `send()`'s — a streaming call is declared by
        // the handler's return type, not by the request.
        let mut msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
            "id": id,
        });
        if !metadata.is_empty() {
            msg["metadata"] = serde_json::json!(metadata);
        }
        let mut frame = msg.to_string();
        frame.push('\n');

        if let Err(e) = inner.writer.lock().await.write_all(frame.as_bytes()).await {
            inner.pending.lock().await.remove(&id);
            self.invalidate().await;
            return Err(RpcClientError::Transport(e.to_string()));
        }

        Ok(stream)
    }

    async fn emit(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<(), RpcClientError> {
        let inner = self.get_or_connect().await?;

        // No id field — server sends no reply.
        let mut msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
        });
        if !metadata.is_empty() {
            msg["metadata"] = serde_json::json!(metadata);
        }
        let mut frame = msg.to_string();
        frame.push('\n');

        let write_result = inner.writer.lock().await.write_all(frame.as_bytes()).await;
        match write_result {
            Ok(()) => Ok(()),
            Err(e) => {
                drop(inner);
                self.invalidate().await;
                Err(RpcClientError::Transport(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// After `invalidate()`, the next `get_or_connect()` call must establish
    /// a fresh TCP connection. Compare the OS-assigned local ports — they
    /// cannot match while both connections are alive.
    #[tokio::test]
    async fn invalidate_rebuilds_connection_on_next_call() {
        // Stand up a dummy server that just accepts connections.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });

        let transport = TcpClientTransport::new("127.0.0.1", port);

        let inner1 = transport.get_or_connect().await.unwrap();
        let local1 = inner1.writer.lock().await.local_addr().unwrap();

        transport.invalidate().await;

        let inner2 = transport.get_or_connect().await.unwrap();
        let local2 = inner2.writer.lock().await.local_addr().unwrap();

        assert_ne!(
            local1.port(),
            local2.port(),
            "reconnect should bind a new ephemeral port"
        );
    }
}
