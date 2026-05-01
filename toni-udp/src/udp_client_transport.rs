use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex, OnceCell};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

/// Maximum UDP datagram payload (theoretical max minus IPv4 + UDP headers).
const MAX_DATAGRAM: usize = 65_507;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Inner {
    socket: Arc<UdpSocket>,
    // Correlation id → channel waiting for the server's reply.
    pending: Mutex<HashMap<String, oneshot::Sender<Result<RpcData, RpcClientError>>>>,
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
/// # Caveats (v1)
///
/// - **No retries**: a lost datagram surfaces as `RpcClientError::Timeout`.
///   Wrap calls in your own retry policy if needed.
/// - **No reconnect**: the socket is created once; if the OS reports a fatal
///   error the next call returns `RpcClientError::Transport`.
/// - **Size-bound**: payloads above ~65 KiB are rejected with a transport
///   error before sending.
///
/// # Example
///
/// ```rust,no_run
/// provider_value!(
///     "ORDERS_CLIENT",
///     toni::RpcClient::new(toni_udp::UdpClientTransport::new("127.0.0.1", 4000))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
pub struct UdpClientTransport {
    host: String,
    port: u16,
    timeout: Duration,
    inner: OnceCell<Arc<Inner>>,
}

impl UdpClientTransport {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            timeout: Duration::from_secs(5),
            inner: OnceCell::new(),
        }
    }

    /// Override the request-response timeout (default: 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn get_or_connect(&self) -> Result<Arc<Inner>, RpcClientError> {
        self.inner
            .get_or_try_init(|| async {
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
                let inner = Arc::new(Inner {
                    socket: socket.clone(),
                    pending: Mutex::new(HashMap::new()),
                });

                tokio::spawn(reader_loop(socket, inner.clone()));

                tracing::info!(addr, "UdpClientTransport connected");
                Ok(inner)
            })
            .await
            .cloned()
    }
}

async fn reader_loop(socket: Arc<UdpSocket>, inner: Arc<Inner>) {
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
                if let Some(tx) = pending.remove(id) {
                    let result = if let Some(err) = msg.get("err") {
                        let message = err["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        let status = err["status"].as_str().unwrap_or("error").to_string();
                        Err(RpcClientError::Remote { message, status })
                    } else {
                        Ok(RpcData::Json(msg["response"].clone()))
                    };
                    let _ = tx.send(result);
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "UdpClientTransport recv error");
                break;
            }
        }
    }

    // Drain all pending requests so callers don't hang indefinitely.
    let mut pending = inner.pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(RpcClientError::Transport(
            "socket closed".to_string(),
        )));
    }

    tracing::debug!("UdpClientTransport socket closed");
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

    async fn send(&self, pattern: &str, data: RpcData) -> Result<RpcData, RpcClientError> {
        let inner = self.get_or_connect().await?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();

        let msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
            "id": id,
        });
        let frame = msg.to_string().into_bytes();

        if frame.len() > MAX_DATAGRAM {
            return Err(RpcClientError::Transport(format!(
                "payload {} bytes exceeds UDP max ({})",
                frame.len(),
                MAX_DATAGRAM
            )));
        }

        let (tx, rx) = oneshot::channel();
        inner.pending.lock().await.insert(id.clone(), tx);

        if let Err(e) = inner.socket.send(&frame).await {
            inner.pending.lock().await.remove(&id);
            return Err(RpcClientError::Transport(e.to_string()));
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcClientError::Transport("socket closed".to_string())),
            Err(_) => {
                inner.pending.lock().await.remove(&id);
                Err(RpcClientError::Timeout)
            }
        }
    }

    async fn emit(&self, pattern: &str, data: RpcData) -> Result<(), RpcClientError> {
        let inner = self.get_or_connect().await?;

        // No id field — server sends no reply.
        let msg = serde_json::json!({
            "pattern": pattern,
            "data": data_to_json(data),
        });
        let frame = msg.to_string().into_bytes();

        if frame.len() > MAX_DATAGRAM {
            return Err(RpcClientError::Transport(format!(
                "payload {} bytes exceeds UDP max ({})",
                frame.len(),
                MAX_DATAGRAM
            )));
        }

        inner
            .socket
            .send(&frame)
            .await
            .map(|_| ())
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }
}
