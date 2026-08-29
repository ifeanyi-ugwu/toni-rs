use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::FutureExt;
use tokio::net::UdpSocket;
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use toni::rpc::wire;
use toni::{async_trait, RpcAdapter, RpcCallInfo, RpcData, RpcMessageCallbacks};
use tracing::Instrument;

/// Maximum UDP datagram payload (theoretical max minus IPv4 + UDP headers).
const MAX_DATAGRAM: usize = 65_507;

const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the adapter gets its socket. The datagram counterpart of
/// [`toni::BindTarget`], which is TCP-typed and so cannot carry a
/// `UdpSocket`; private because the two constructors cover the whole surface.
enum UdpTarget {
    Addr { hostname: String, port: u16 },
    Socket(std::net::UdpSocket),
}

impl UdpTarget {
    fn into_std_socket(self) -> std::io::Result<std::net::UdpSocket> {
        match self {
            UdpTarget::Addr { hostname, port } => {
                std::net::UdpSocket::bind((hostname.as_str(), port))
            }
            UdpTarget::Socket(socket) => Ok(socket),
        }
    }
}

impl std::fmt::Display for UdpTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UdpTarget::Addr { hostname, port } => write!(f, "{hostname}:{port}"),
            UdpTarget::Socket(socket) => match socket.local_addr() {
                Ok(addr) => write!(f, "pre-bound socket on {addr}"),
                Err(_) => write!(f, "pre-bound socket"),
            },
        }
    }
}

/// UDP transport adapter for the Toni RPC gateway.
///
/// One datagram = one JSON message. Inbound:
///
/// ```json
/// {"pattern":"order.create","data":{...},"id":"<correlation-id>"}
/// ```
///
/// `id` is optional. When present and the handler returns a reply
/// (request-response pattern), the response is sent back to the source
/// address:
///
/// ```json
/// {"id":"<correlation-id>","response":{...}}
/// ```
///
/// Fire-and-forget events (no `id`, or handlers declared with `#[event_pattern]`)
/// produce no datagram on the wire.
///
/// # Caveats (v1)
///
/// - **Unreliable**: datagrams can be lost, duplicated, or reordered. The
///   client transport relies on a request timeout — there are no automatic
///   retries.
/// - **Size-bound**: payloads larger than ~65 KiB cannot fit in a single
///   datagram. Oversized inbound datagrams are truncated by the kernel and
///   the truncated frame is logged and dropped.
///
/// # Graceful shutdown
///
/// On shutdown the receive loop stops accepting new datagrams, then in-flight
/// per-datagram tasks are awaited up to a
/// configurable drain timeout (default 10 s). Tasks still running after the
/// timeout are aborted. Override with [`UdpAdapter::with_drain_timeout`].
///
/// # Backpressure
///
/// By default the adapter spawns one task per inbound datagram with no
/// upper bound. Set a cap with [`UdpAdapter::with_max_inflight`] to
/// reject datagrams that would exceed it. Rejected request-response
/// datagrams get an `"overloaded"` error frame back; fire-and-forget
/// datagrams are dropped with a log line.
pub struct UdpAdapter {
    target: Option<UdpTarget>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
    socket: Option<Arc<UdpSocket>>,
    local_addr: Option<SocketAddr>,
    shutdown_tx: Arc<watch::Sender<bool>>,
    drain_timeout: Option<Duration>,
    inflight: Option<Arc<Semaphore>>,
}

impl UdpAdapter {
    /// Listen on `host:port`. Port 0 asks the OS for a free port; read the
    /// assigned address back from `BoundAdapters::rpc`.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self::with_target(UdpTarget::Addr {
            hostname: host.into(),
            port,
        })
    }

    /// Receive on a socket the caller already bound, instead of binding one.
    ///
    /// Datagram sockets are what systemd hands over for a `ListenDatagram=`
    /// unit; pair with `listenfd`'s `take_udp_socket` to claim an inherited
    /// descriptor. Unlike TCP there is no accept backlog, so a socket held
    /// across a restart buys queueing in the receive buffer rather than
    /// ICMP port-unreachable replies.
    pub fn from_socket(socket: std::net::UdpSocket) -> Self {
        Self::with_target(UdpTarget::Socket(socket))
    }

    fn with_target(target: UdpTarget) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            target: Some(target),
            callbacks: None,
            socket: None,
            local_addr: None,
            shutdown_tx: Arc::new(tx),
            drain_timeout: Some(DEFAULT_DRAIN_TIMEOUT),
            inflight: None,
        }
    }

    /// Set how long `close()` waits for in-flight datagram tasks to finish
    /// before aborting them. Pass `None` to wait without bound.
    pub fn with_drain_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.drain_timeout = timeout.into();
        self
    }

    /// Cap the number of concurrently running datagram handlers. Inbound
    /// datagrams over the cap are rejected immediately with an
    /// `"overloaded"` error frame (or dropped, for fire-and-forget).
    /// Default: unbounded.
    pub fn with_max_inflight(mut self, max: usize) -> Self {
        self.inflight = Some(Arc::new(Semaphore::new(max)));
        self
    }
}

#[async_trait]
impl RpcAdapter for UdpAdapter {
    fn register_handlers(
        &mut self,
        _patterns: &[String],
        callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<()> {
        // Bind synchronously so `app.start().await` surfaces a port-in-use
        // failure as `Err` instead of panicking inside the spawned recv loop.
        let target = self
            .target
            .take()
            .context("UdpAdapter: register_handlers() called more than once")?;
        let described = target.to_string();
        let std_socket = target
            .into_std_socket()
            .with_context(|| format!("UdpAdapter: failed to listen on {described}"))?;
        std_socket
            .set_nonblocking(true)
            .context("UdpAdapter: failed to set socket nonblocking")?;
        let socket = UdpSocket::from_std(std_socket)
            .context("UdpAdapter: failed to register socket with the tokio runtime")?;
        let local_addr = socket
            .local_addr()
            .context("UdpAdapter: failed to read local address from socket")?;

        self.callbacks = Some(callbacks);
        self.socket = Some(Arc::new(socket));
        self.local_addr = Some(local_addr);
        Ok(())
    }

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::RpcLifecycleHandle> {
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");
        let socket = self
            .socket
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");
        let local_addr = self.local_addr;
        let drain_timeout = self.drain_timeout;
        let inflight = self.inflight.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let serve = Box::pin(async move {
            let addr = socket
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            tracing::info!(addr, "UdpAdapter listening");

            // Tracks every spawned per-datagram task so close() can drain them.
            let tasks: Arc<Mutex<JoinSet<()>>> = Arc::new(Mutex::new(JoinSet::new()));

            let mut buf = vec![0u8; MAX_DATAGRAM];
            loop {
                tokio::select! {
                    biased;
                    _ = wait_for_shutdown(&mut shutdown_rx) => {
                        tracing::info!(addr, "UdpAdapter shutting down");
                        break;
                    }
                    res = socket.recv_from(&mut buf) => match res {
                        Ok((n, src)) => {
                            // recv_from returns the full datagram size if it fit.
                            // If it equals the buffer it may have been truncated;
                            // we accept it but warn.
                            if n == MAX_DATAGRAM {
                                tracing::warn!(addr = %src, "UdpAdapter possibly truncated datagram");
                            }

                            let payload = buf[..n].to_vec();
                            let socket = socket.clone();
                            let callbacks = callbacks.clone();

                            // Backpressure: claim a permit before spawning.
                            // If the cap is full, reject inline so the caller
                            // learns immediately. Fire-and-forget is dropped.
                            let permit: Option<OwnedSemaphorePermit> = match &inflight {
                                Some(sem) => match sem.clone().try_acquire_owned() {
                                    Ok(p) => Some(p),
                                    Err(_) => {
                                        reject_overloaded(&socket, src, &payload).await;
                                        continue;
                                    }
                                },
                                None => None,
                            };

                            tasks.lock().await.spawn(async move {
                                let _permit = permit;
                                handle_datagram(socket, src, payload, callbacks).await;
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "UdpAdapter recv error");
                        }
                    }
                }
            }

            drain_tasks(tasks, drain_timeout, &addr).await;
        });

        let shutdown_tx = self.shutdown_tx.clone();
        Ok(toni::RpcLifecycleHandle::new(
            local_addr,
            serve,
            move || async move {
                let _ = shutdown_tx.send(true);
                Ok(())
            },
        ))
    }
}

// `watch::Receiver::wait_for` resolves to `Result<Ref<'_, T>, _>`. The
// `Ref` guard isn't `Send`, which forces the whole `serve` future to be
// `!Send` whenever we hold a `Mutex` lock across it. Wrapping the await
// here drops the guard inside the helper so the outer scope sees `()`.
async fn wait_for_shutdown(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

async fn drain_tasks(tasks: Arc<Mutex<JoinSet<()>>>, drain_timeout: Option<Duration>, addr: &str) {
    let drain = async {
        let mut js = tasks.lock().await;
        while js.join_next().await.is_some() {}
    };

    match drain_timeout {
        Some(d) => {
            if tokio::time::timeout(d, drain).await.is_err() {
                let mut js = tasks.lock().await;
                let aborted = js.len();
                if aborted > 0 {
                    tracing::warn!(
                        addr,
                        aborted,
                        timeout_ms = d.as_millis() as u64,
                        "UdpAdapter drain timed out; aborting in-flight tasks"
                    );
                    js.abort_all();
                    while js.join_next().await.is_some() {}
                }
            }
        }
        None => drain.await,
    }
}

/// Send an `"overloaded"` error frame to the source if the inbound datagram
/// has an `id` (request-response). Fire-and-forget messages are dropped with
/// a log line — there's no caller waiting to be notified.
async fn reject_overloaded(socket: &UdpSocket, src: std::net::SocketAddr, payload: &[u8]) {
    let id: Option<String> = serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v["id"].as_str().map(|s| s.to_string()));

    let Some(id) = id else {
        tracing::warn!(addr = %src, "UdpAdapter dropping fire-and-forget datagram: at capacity");
        return;
    };

    let frame = serde_json::json!({
        "id": id,
        "err": { "message": "server at capacity", "status": "overloaded" }
    });
    let bytes = frame.to_string().into_bytes();
    if let Err(e) = socket.send_to(&bytes, src).await {
        tracing::error!(error = %e, "UdpAdapter send error on overload reject");
    }
}

async fn handle_datagram(
    socket: Arc<UdpSocket>,
    src: std::net::SocketAddr,
    payload: Vec<u8>,
    callbacks: Arc<RpcMessageCallbacks>,
) {
    let msg: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(addr = %src, error = %e, "UdpAdapter JSON parse error");
            return;
        }
    };

    let pattern = msg["pattern"].as_str().unwrap_or("").to_string();
    let data = RpcData::Json(msg["data"].clone());
    // `id` present → caller expects a response
    let id = msg["id"].as_str().map(|s| s.to_string());
    // Per-request span — covers the user handler invocation and the
    // response-write so any events from the handler inherit the request
    // context. Built before `pattern` is moved into ctx.
    let span = tracing::info_span!(
        "rpc.request",
        transport = "udp",
        pattern = %pattern,
        id = ?id,
        peer = %src,
    );
    let mut ctx = RpcCallInfo::new(pattern);
    ctx.headers = extract_metadata(&msg);

    async move {
        let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
            .catch_unwind()
            .await;

        let Some(id) = id else {
            if outcome.is_err() {
                tracing::error!("RPC handler panicked on fire-and-forget message");
            }
            return;
        };

        let mut payload_json = match outcome {
            Err(_) => {
                tracing::error!("RPC handler panicked; returning error to caller");
                wire::frame_panic().into_json_value()
            }
            Ok(outcome) => wire::frame_response(outcome).into_json_value(),
        };
        payload_json["id"] = serde_json::Value::String(id);

        let bytes = payload_json.to_string().into_bytes();
        if bytes.len() > MAX_DATAGRAM {
            tracing::error!(
                len = bytes.len(),
                "UdpAdapter response exceeds max datagram size; dropping"
            );
            return;
        }

        if let Err(e) = socket.send_to(&bytes, src).await {
            tracing::error!(error = %e, "UdpAdapter send error");
        }
    }
    .instrument(span)
    .await;
}

/// Pull the optional `metadata` object from an inbound frame into the flat
/// string map surfaced as `RpcContext` metadata. Non-string values are skipped.
fn extract_metadata(msg: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    if let Some(map) = msg.get("metadata").and_then(|m| m.as_object()) {
        for (key, value) in map {
            if let Some(s) = value.as_str() {
                metadata.insert(key.clone(), s.to_string());
            }
        }
    }
    metadata
}
