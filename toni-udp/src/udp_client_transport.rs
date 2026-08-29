use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::SinkExt;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Mutex};
use toni::rpc::wire::{self, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

/// Maximum UDP datagram payload (theoretical max minus IPv4 + UDP headers).
const MAX_DATAGRAM: usize = 65_507;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One awaited call in the correlation map.
enum PendingSlot {
    /// A `send()` call: consumed by the first datagram carrying its id.
    Single(oneshot::Sender<Result<RpcData, RpcClientError>>),
    /// An `open_stream()` call: fed frame by frame until the end marker,
    /// which removes the entry and thereby closes this sender.
    Stream(mpsc::UnboundedSender<Result<RpcData, RpcClientError>>),
}

struct Inner {
    socket: Arc<UdpSocket>,
    // Correlation id → channel waiting for the server's reply.
    pending: Mutex<HashMap<String, PendingSlot>>,
    // Cancel notices for abandoned streaming calls; drained by cancel_loop,
    // which removes the pending entry and sends the cancel datagram.
    cancel_tx: mpsc::UnboundedSender<String>,
}

/// UDP transport for [`RpcClient`].
///
/// Binds a single ephemeral UDP socket and `connect`s it to the remote
/// address so subsequent reads/writes use a fixed peer. Request-response
/// uses a monotonic correlation id; the background reader loop matches each
/// incoming `{"id":..., "response":...}` or `{"id":..., "err":...}` datagram
/// to the waiting caller and delivers it via an in-memory channel.
///
/// Fire-and-forget (`emit`) sends a datagram with no `id` field and returns
/// as soon as `send` completes.
///
/// A streaming call (`open_stream`, ADR-0032) sends the same request datagram;
/// the reader loop feeds `{"id":…,"stream":…}` datagrams to the caller's
/// [`RpcReplyStream`] until `{"id":…,"end":true}`. The configured timeout
/// bounds the gap to the next frame, the first included, and there are no
/// retries for streams. Dropping the reply stream before its end sends
/// `{"id":…,"cancel":true}`, aborting the call on the server.
///
/// # Caveats
///
/// - **No retries**: a lost datagram surfaces as `RpcClientError::Timeout`.
///   Wrap calls in your own retry policy if needed.
/// - **Size-bound**: payloads above ~65 KiB are rejected with a transport
///   error before sending.
///
/// The transport rebinds lazily on the next call after a fatal socket error
/// (recv loop exits, `send` returns an error). Pending requests at the moment
/// of failure surface as `RpcClientError::Transport("socket closed")`.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "ORDERS_CLIENT",
///     toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", 4000))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
// Slot type shared between the public transport and the background reader
// loop. The reader clears the slot on exit so the next caller rebuilds the
// socket — this is the lazy-reconnect path.
type Slot = Arc<Mutex<Option<Arc<Inner>>>>;

pub struct UdpClientTransport {
    host: String,
    port: u16,
    timeout: Duration,
    retries: u32,
    retry_backoff: Duration,
    slot: Slot,
}

impl UdpClientTransport {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            timeout: Duration::from_secs(5),
            retries: 0,
            retry_backoff: Duration::from_millis(100),
            slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Per-attempt request-response timeout (default: 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Re-send `n` additional times if `send` times out. Each retry uses a
    /// fresh correlation id so a delayed reply for an earlier attempt is
    /// silently dropped instead of satisfying a later one. Default: `0`
    /// (off). Total wall time is bounded by
    /// `(retries + 1) * timeout + retries * backoff`.
    pub fn with_retries(mut self, n: u32) -> Self {
        self.retries = n;
        self
    }

    /// Constant delay between retry attempts. Default: 100 ms. Ignored when
    /// `retries == 0`.
    pub fn with_retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }

    async fn get_or_connect(&self) -> Result<Arc<Inner>, RpcClientError> {
        let mut guard = self.slot.lock().await;
        if let Some(inner) = guard.as_ref() {
            return Ok(inner.clone());
        }

        // Bind to an OS-assigned ephemeral port. Use IPv4 wildcard;
        // `connect` below pins the peer.
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| RpcClientError::Transport(e.to_string()))?;
        let addr = format!("{}:{}", self.host, self.port);
        socket
            .connect(&addr)
            .await
            .map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let socket = Arc::new(socket);
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            socket: socket.clone(),
            pending: Mutex::new(HashMap::new()),
            cancel_tx,
        });

        tokio::spawn(reader_loop(socket, inner.clone(), self.slot.clone()));
        // Weak: the loop must not keep the socket alive once the slot is
        // cleared and every caller is gone — `Inner` holds the sender, so a
        // strong reference here would be a cycle.
        tokio::spawn(cancel_loop(cancel_rx, Arc::downgrade(&inner)));

        tracing::info!(addr, "UdpClientTransport connected");
        *guard = Some(inner.clone());
        Ok(inner)
    }

    /// Drop the cached socket so the next call rebuilds. Safe to call
    /// concurrently — the reader loop and `send` paths race to clear, but
    /// `take()` is idempotent.
    async fn invalidate(&self) {
        self.slot.lock().await.take();
    }
}

async fn reader_loop(socket: Arc<UdpSocket>, inner: Arc<Inner>, slot: Slot) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    loop {
        match socket.recv(&mut buf).await {
            Ok(n) => {
                let msg: serde_json::Value = match serde_json::from_slice(&buf[..n]) {
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
                        let result = match wire::parse_reply_frame(&buf[..n]) {
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
                    Some(PendingSlot::Stream(tx)) => match wire::parse_reply_frame(&buf[..n]) {
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
                    },
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "UdpClientTransport recv error");
                break;
            }
        }
    }

    // Clear the cached socket so the next caller rebuilds.
    slot.lock().await.take();

    // Drain all pending requests so callers don't hang indefinitely.
    let mut pending = inner.pending.lock().await;
    for (_, slot) in pending.drain() {
        let closed = || RpcClientError::Transport("socket closed".to_string());
        match slot {
            PendingSlot::Single(tx) => {
                let _ = tx.send(Err(closed()));
            }
            PendingSlot::Stream(tx) => {
                let _ = tx.send(Err(closed()));
            }
        }
    }

    tracing::debug!("UdpClientTransport socket closed");
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
            // End marker, socket loss, or cancel — the entry is gone and the
            // sender with it.
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
/// server with a `{"id", "cancel": true}` datagram.
async fn cancel_loop(mut rx: mpsc::UnboundedReceiver<String>, weak: std::sync::Weak<Inner>) {
    while let Some(id) = rx.recv().await {
        let Some(inner) = weak.upgrade() else { break };
        inner.pending.lock().await.remove(&id);
        let frame = serde_json::json!({ "id": id, "cancel": true }).to_string();
        if let Err(e) = inner.socket.send(frame.as_bytes()).await {
            tracing::debug!(error = %e, "UdpClientTransport cancel send failed");
        }
    }
}

fn data_to_json(data: RpcData) -> serde_json::Value {
    match data {
        RpcData::Json(v) => v,
        RpcData::Text(s) => serde_json::Value::String(s),
        // UDP wire format is JSON; binary payloads are not supported.
        RpcData::Binary(_) => serde_json::Value::Null,
    }
}

#[async_trait]
impl RpcClientTransport for UdpClientTransport {
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
        // Pre-serialize the data once. Each attempt rewraps it with a fresh
        // correlation id so a late reply for an earlier attempt is dropped by
        // the reader loop instead of satisfying a later one.
        let data_json = data_to_json(data);

        let attempts = self.retries.saturating_add(1);
        let mut last_timeout: Option<RpcClientError> = None;

        for attempt in 0..attempts {
            if attempt > 0 {
                tokio::time::sleep(self.retry_backoff).await;
            }

            let inner = self.get_or_connect().await?;
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();

            let mut msg = serde_json::json!({
                "pattern": pattern,
                "data": data_json,
                "id": id,
            });
            if !metadata.is_empty() {
                msg["metadata"] = serde_json::json!(metadata);
            }
            let frame = msg.to_string().into_bytes();

            if frame.len() > MAX_DATAGRAM {
                return Err(RpcClientError::Transport(format!(
                    "payload {} bytes exceeds UDP max ({})",
                    frame.len(),
                    MAX_DATAGRAM
                )));
            }

            let (tx, rx) = oneshot::channel();
            inner
                .pending
                .lock()
                .await
                .insert(id.clone(), PendingSlot::Single(tx));

            if let Err(e) = inner.socket.send(&frame).await {
                inner.pending.lock().await.remove(&id);
                // Fatal-looking — drop the cached socket so the next call
                // rebuilds rather than retrying on a half-broken handle.
                self.invalidate().await;
                return Err(RpcClientError::Transport(e.to_string()));
            }

            match tokio::time::timeout(self.timeout, rx).await {
                Ok(Ok(result)) => return result,
                Ok(Err(_)) => return Err(RpcClientError::Transport("socket closed".to_string())),
                Err(_) => {
                    inner.pending.lock().await.remove(&id);
                    last_timeout = Some(RpcClientError::Timeout);
                    // Fall through to next attempt.
                }
            }
        }

        Err(last_timeout.unwrap_or(RpcClientError::Timeout))
    }

    async fn open_stream(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcReplyStream, RpcClientError> {
        let inner = self.get_or_connect().await?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();

        // No retries for a stream: a re-sent request could double-start the
        // producer. A lost request datagram surfaces as the first frame's
        // timeout.
        let mut msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
            "id": id,
        });
        if !metadata.is_empty() {
            msg["metadata"] = serde_json::json!(metadata);
        }
        let frame = msg.to_string().into_bytes();
        if frame.len() > MAX_DATAGRAM {
            return Err(RpcClientError::Transport(format!(
                "payload {} bytes exceeds UDP max ({})",
                frame.len(),
                MAX_DATAGRAM
            )));
        }

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

        if let Err(e) = inner.socket.send(&frame).await {
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
        let frame = msg.to_string().into_bytes();

        if frame.len() > MAX_DATAGRAM {
            return Err(RpcClientError::Transport(format!(
                "payload {} bytes exceeds UDP max ({})",
                frame.len(),
                MAX_DATAGRAM
            )));
        }

        match inner.socket.send(&frame).await {
            Ok(_) => Ok(()),
            Err(e) => {
                self.invalidate().await;
                Err(RpcClientError::Transport(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After `invalidate()`, the next `get_or_connect()` call must bind a
    /// fresh socket. Compare the OS-assigned local ports — they cannot match
    /// while both sockets are alive.
    #[tokio::test]
    async fn invalidate_rebuilds_socket_on_next_call() {
        // Bind a dummy server so the client has a real peer to connect to.
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();

        let transport = UdpClientTransport::new("127.0.0.1", port);

        let inner1 = transport.get_or_connect().await.unwrap();
        let local1 = inner1.socket.local_addr().unwrap();

        transport.invalidate().await;

        let inner2 = transport.get_or_connect().await.unwrap();
        let local2 = inner2.socket.local_addr().unwrap();

        assert_ne!(
            local1.port(),
            local2.port(),
            "reconnect should bind a new ephemeral port"
        );
    }
}
