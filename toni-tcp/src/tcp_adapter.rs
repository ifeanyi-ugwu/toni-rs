use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::FutureExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use toni::{async_trait, RpcAdapter, RpcContext, RpcData, RpcError, RpcMessageCallbacks};

const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP transport adapter for the Toni RPC gateway.
///
/// Uses newline-delimited JSON. Each message is a single JSON object followed
/// by `\n`. Inbound:
///
/// ```json
/// {"pattern":"order.create","data":{...},"id":"<correlation-id>"}
/// ```
///
/// `id` is optional. When present and the handler returns a reply
/// (request-response pattern), the response is written back:
///
/// ```json
/// {"id":"<correlation-id>","response":{...}}
/// ```
///
/// Fire-and-forget events (no `id`, or handlers declared with `#[event_pattern]`)
/// produce no response on the wire.
///
/// # Graceful shutdown
///
/// On [`close`](RpcAdapter::close), the accept loop stops, connection
/// handlers stop reading new lines, and in-flight per-message tasks are
/// awaited up to a configurable drain timeout (default 10 s). Tasks still
/// running after the timeout are aborted. Override the timeout — including
/// disabling it — with [`TcpAdapter::with_drain_timeout`].
///
/// # Backpressure
///
/// By default the adapter spawns one task per inbound message with no
/// upper bound. Set a cap with [`TcpAdapter::with_max_inflight`] to
/// reject requests that would exceed it. Rejected request-response
/// messages get an `"overloaded"` error frame back; fire-and-forget
/// messages are dropped with a log line.
pub struct TcpAdapter {
    host: String,
    port: u16,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
    listener: Option<TcpListener>,
    local_addr: Option<SocketAddr>,
    shutdown_tx: Arc<watch::Sender<bool>>,
    drain_timeout: Option<Duration>,
    inflight: Option<Arc<Semaphore>>,
}

impl TcpAdapter {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            host: host.into(),
            port,
            callbacks: None,
            listener: None,
            local_addr: None,
            shutdown_tx: Arc::new(tx),
            drain_timeout: Some(DEFAULT_DRAIN_TIMEOUT),
            inflight: None,
        }
    }

    /// Set how long `close()` waits for in-flight requests to finish before
    /// aborting them. Pass `None` to wait without bound.
    pub fn with_drain_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.drain_timeout = timeout.into();
        self
    }

    /// Cap the number of concurrently running handler tasks across all
    /// connections. Inbound requests over the cap are rejected immediately
    /// with an `"overloaded"` error frame (or dropped, for fire-and-forget).
    /// Default: unbounded.
    pub fn with_max_inflight(mut self, max: usize) -> Self {
        self.inflight = Some(Arc::new(Semaphore::new(max)));
        self
    }
}

#[async_trait]
impl RpcAdapter for TcpAdapter {
    fn bind(&mut self, _patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
        // Bind synchronously so port-in-use surfaces as `Err` from
        // `app.start()` instead of panicking inside the spawned accept loop.
        let addr = format!("{}:{}", self.host, self.port);
        let std_listener = std::net::TcpListener::bind(&addr)
            .with_context(|| format!("TcpAdapter: failed to bind {}", addr))?;
        std_listener
            .set_nonblocking(true)
            .context("TcpAdapter: failed to set listener nonblocking")?;
        let listener = TcpListener::from_std(std_listener)
            .context("TcpAdapter: failed to register listener with the tokio runtime")?;
        let local_addr = listener
            .local_addr()
            .context("TcpAdapter: failed to read local address from listener")?;

        self.callbacks = Some(callbacks);
        self.listener = Some(listener);
        self.local_addr = Some(local_addr);
        Ok(())
    }

    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        let callbacks = self
            .callbacks
            .take()
            .expect("bind() must be called before serve()");
        let listener = self
            .listener
            .take()
            .expect("bind() must be called before serve()");
        let shutdown_tx = self.shutdown_tx.clone();
        let drain_timeout = self.drain_timeout;
        let inflight = self.inflight.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();

        Ok(Box::pin(async move {
            let addr = listener
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            tracing::info!(addr, "TcpAdapter listening");

            // All spawned work — connection handlers and per-message tasks —
            // is tracked here so close() can drain it.
            let tasks: Arc<Mutex<JoinSet<()>>> = Arc::new(Mutex::new(JoinSet::new()));

            loop {
                tokio::select! {
                    biased;
                    _ = wait_for_shutdown(&mut shutdown_rx) => {
                        tracing::info!(addr, "TcpAdapter shutting down");
                        break;
                    }
                    res = listener.accept() => match res {
                        Ok((stream, peer)) => {
                            let callbacks = callbacks.clone();
                            let tasks_for_conn = tasks.clone();
                            let conn_shutdown_rx = shutdown_tx.subscribe();
                            let inflight = inflight.clone();
                            tasks.lock().await.spawn(handle_connection(
                                stream,
                                peer,
                                callbacks,
                                tasks_for_conn,
                                conn_shutdown_rx,
                                inflight,
                            ));
                        }
                        Err(e) => tracing::error!(error = %e, "TcpAdapter accept error"),
                    }
                }
            }

            drain_tasks(tasks, drain_timeout, &addr).await;
        }))
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    async fn close(&mut self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        Ok(())
    }
}

// `watch::Receiver::wait_for` resolves to `Result<Ref<'_, T>, _>`. The
// `Ref` guard isn't `Send`, which forces the whole `serve` future to be
// `!Send` whenever we hold a `Mutex` lock across it. Wrapping the await
// here drops the guard inside the helper so the outer scope sees `()`.
async fn wait_for_shutdown(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

async fn drain_tasks(
    tasks: Arc<Mutex<JoinSet<()>>>,
    drain_timeout: Option<Duration>,
    addr: &str,
) {
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
                        "TcpAdapter drain timed out; aborting in-flight tasks"
                    );
                    js.abort_all();
                    while js.join_next().await.is_some() {}
                }
            }
        }
        None => drain.await,
    }
}

fn error_status(e: &RpcError) -> &'static str {
    match e {
        RpcError::PatternNotFound(_) => "not_found",
        RpcError::Forbidden(_) => "forbidden",
        RpcError::Internal(_) => "error",
    }
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    callbacks: Arc<RpcMessageCallbacks>,
    tasks: Arc<Mutex<JoinSet<()>>>,
    mut shutdown_rx: watch::Receiver<bool>,
    inflight: Option<Arc<Semaphore>>,
) {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    // Shared across per-message spawns on this connection
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    loop {
        line.clear();
        let read = tokio::select! {
            biased;
            // Stop reading new requests on shutdown; in-flight per-message
            // tasks already in `tasks` will be drained by serve().
            _ = wait_for_shutdown(&mut shutdown_rx) => break,
            res = reader.read_line(&mut line) => res,
        };

        match read {
            Ok(0) => break, // clean close
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let msg: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(addr = %addr, error = %e, "TcpAdapter JSON parse error");
                        continue;
                    }
                };

                let pattern = msg["pattern"].as_str().unwrap_or("").to_string();
                let data = RpcData::Json(msg["data"].clone());
                // `id` present → caller expects a response
                let id = msg["id"].as_str().map(|s| s.to_string());
                let ctx = RpcContext::new(pattern);

                // Backpressure: try to claim a permit before spawning. If
                // the cap is full, reject inline (write is cheap) so the
                // caller learns immediately instead of queuing forever.
                let permit: Option<OwnedSemaphorePermit> = match &inflight {
                    Some(sem) => match sem.clone().try_acquire_owned() {
                        Ok(p) => Some(p),
                        Err(_) => {
                            match &id {
                                Some(id) => {
                                    let frame = serde_json::json!({
                                        "id": id,
                                        "err": {
                                            "message": "server at capacity",
                                            "status": "overloaded"
                                        }
                                    });
                                    let mut line = frame.to_string();
                                    line.push('\n');
                                    let mut w = writer.lock().await;
                                    if let Err(e) = w.write_all(line.as_bytes()).await {
                                        tracing::error!(error = %e, "TcpAdapter write error on overload reject");
                                    }
                                }
                                None => {
                                    tracing::warn!(
                                        addr = %addr,
                                        "TcpAdapter dropping fire-and-forget message: at capacity"
                                    );
                                }
                            }
                            continue;
                        }
                    },
                    None => None,
                };

                let callbacks = callbacks.clone();
                let writer = writer.clone();

                tasks.lock().await.spawn(async move {
                    // Permit is held for the lifetime of this task; releases
                    // on drop when the task completes or is aborted.
                    let _permit = permit;
                    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
                        .catch_unwind()
                        .await;

                    let Some(id) = id else {
                        if outcome.is_err() {
                            tracing::error!("RPC handler panicked on fire-and-forget message");
                        }
                        return;
                    };

                    let payload_json = match outcome {
                        Err(_) => {
                            tracing::error!("RPC handler panicked; returning error to caller");
                            serde_json::json!({
                                "id": id,
                                "err": { "message": "internal server error", "status": "error" }
                            })
                        }
                        Ok(outcome) => match outcome {
                            Ok(Some(reply)) => {
                                let v = match reply {
                                    RpcData::Json(v) => v,
                                    RpcData::Text(s) => serde_json::Value::String(s),
                                    RpcData::Binary(_) => serde_json::Value::Null,
                                };
                                serde_json::json!({ "id": id, "response": v })
                            }
                            Ok(None) => {
                                // Handler is fire-and-forget (#[event_pattern]) but
                                // caller sent an id — send an explicit ack so caller
                                // can close the pending request rather than timing out.
                                serde_json::json!({ "id": id, "response": null })
                            }
                            Err(e) => {
                                let status = error_status(&e);
                                serde_json::json!({
                                    "id": id,
                                    "err": { "message": e.to_string(), "status": status }
                                })
                            }
                        },
                    };

                    let mut line = payload_json.to_string();
                    line.push('\n');

                    let mut w = writer.lock().await;
                    if let Err(e) = w.write_all(line.as_bytes()).await {
                        tracing::error!(error = %e, "TcpAdapter write error");
                    }
                });
            }
            Err(e) => {
                tracing::error!(addr = %addr, error = %e, "TcpAdapter read error");
                break;
            }
        }
    }
}
