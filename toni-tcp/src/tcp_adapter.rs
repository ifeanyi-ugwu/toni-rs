use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::FutureExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use toni::rpc::wire;
use toni::{async_trait, BindTarget, RpcAdapter, RpcCallInfo, RpcData, RpcMessageCallbacks};
use tracing::Instrument;

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
/// A streaming reply (ADR-0032) is item frames followed by an end marker,
/// every frame carrying the call's `id`:
///
/// ```json
/// {"id":"<correlation-id>","stream":{...}}
/// {"id":"<correlation-id>","end":true}
/// ```
///
/// `Binary` items travel base64 under `"stream_b64"`; an abnormal end is
/// `{"id":…,"end":true,"err":{"message","status"}}`. The caller abandons an
/// in-flight call — mid-handler or mid-stream — by sending
/// `{"id":"<correlation-id>","cancel":true}`; the driving task is aborted and
/// the handler's execution hears its cancellation token. A dropped connection
/// cancels everything the connection still had in flight.
///
/// # Graceful shutdown
///
/// On shutdown the accept loop stops, connection
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
    target: Option<BindTarget>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
    listener: Option<TcpListener>,
    local_addr: Option<SocketAddr>,
    shutdown_tx: Arc<watch::Sender<bool>>,
    drain_timeout: Option<Duration>,
    inflight: Option<Arc<Semaphore>>,
}

impl TcpAdapter {
    /// Listen on `host:port`. Port 0 asks the OS for a free port; read the
    /// assigned address back from `BoundAdapters::rpc`.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self::with_target((host.into(), port))
    }

    /// Serve on a socket the caller already bound and put into listening
    /// state, instead of binding one.
    ///
    /// The socket outlives the process that hands it over, which is the point:
    /// a supervisor holding it across restarts (systemd socket activation,
    /// `toni dev --listen`) leaves requests queued in the accept backlog
    /// rather than refused. Pair with the `listenfd` crate to claim an
    /// inherited descriptor.
    pub fn from_listener(listener: std::net::TcpListener) -> Self {
        Self::with_target(listener)
    }

    fn with_target(target: impl Into<BindTarget>) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            target: Some(target.into()),
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
    fn register_handlers(
        &mut self,
        _patterns: &[String],
        callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<()> {
        // Bind synchronously so port-in-use surfaces as `Err` from
        // `app.start()` instead of panicking inside the spawned accept loop.
        let target = self
            .target
            .take()
            .context("TcpAdapter: register_handlers() called more than once")?;
        let described = target.to_string();
        let std_listener = target
            .into_std_listener()
            .with_context(|| format!("TcpAdapter: failed to listen on {described}"))?;
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

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::RpcLifecycleHandle> {
        let callbacks = self
            .callbacks
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");
        let listener = self
            .listener
            .take()
            .expect("register_handlers() must be called before into_lifecycle()");
        let local_addr = self.local_addr;
        let shutdown_tx = self.shutdown_tx.clone();
        let drain_timeout = self.drain_timeout;
        let inflight = self.inflight.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();

        let serve = Box::pin(async move {
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
    // Request-shaped calls on this connection, keyed by id, abortable by a
    // cancel frame or by the peer going away.
    let conn_inflight = wire::Inflight::new();
    let mut peer_gone = false;

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
            Ok(0) => {
                // clean close
                peer_gone = true;
                break;
            }
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

                if msg.get("cancel").and_then(|c| c.as_bool()) == Some(true) {
                    if let Some(id) = msg["id"].as_str() {
                        conn_inflight.cancel(id);
                    }
                    continue;
                }

                let pattern = msg["pattern"].as_str().unwrap_or("").to_string();
                let data = RpcData::Json(msg["data"].clone());
                // `id` present → caller expects a response
                let id = msg["id"].as_str().map(|s| s.to_string());
                // Build the per-request span before `pattern` is moved into ctx.
                // Each spawned handler runs inside this span so all events
                // emitted by user handlers inherit pattern/id/peer fields.
                let span = tracing::info_span!(
                    "rpc.request",
                    transport = "tcp",
                    pattern = %pattern,
                    id = ?id,
                    peer = %addr,
                );
                let mut ctx = RpcCallInfo::new(pattern);
                ctx.headers = extract_metadata(&msg);

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

                // Register a request-shaped call before dispatch, so a cancel
                // frame can arrive as early as the line after the request. The
                // abort handle exists only after the spawn; the read loop is
                // sequential, so the slot is filled before any cancel for this
                // id can be read.
                let (abort_slot, guard) = match &id {
                    Some(id) => {
                        let abort_slot =
                            Arc::new(std::sync::Mutex::new(None::<tokio::task::AbortHandle>));
                        let slot = abort_slot.clone();
                        let guard = conn_inflight.register(id.clone(), move || {
                            if let Some(handle) = slot.lock().unwrap().take() {
                                handle.abort();
                            }
                        });
                        (Some(abort_slot), Some(guard))
                    }
                    None => (None, None),
                };

                let task = async move {
                    // Permit and guard are held for the lifetime of this task;
                    // both release on drop when it completes or is aborted.
                    let _permit = permit;
                    let _guard = guard;
                    let outcome = std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
                        .catch_unwind()
                        .await;

                    let Some(id) = id else {
                        if outcome.is_err() {
                            tracing::error!("RPC handler panicked on fire-and-forget message");
                        }
                        return;
                    };

                    match outcome {
                        Err(_) => {
                            tracing::error!("RPC handler panicked; returning error to caller");
                            write_frame(&writer, wire::frame_panic().into_json_value(), &id).await;
                        }
                        Ok(Ok(toni::RpcHandlerOutput::Stream(stream))) => {
                            wire::drive_reply_stream(stream, |mut frame| {
                                let writer = writer.clone();
                                let id = id.clone();
                                async move {
                                    frame["id"] = serde_json::Value::String(id);
                                    let mut line = frame.to_string();
                                    line.push('\n');
                                    // Lock per frame: a long drain must not
                                    // starve the connection's other replies.
                                    let mut w = writer.lock().await;
                                    w.write_all(line.as_bytes()).await.map_err(|e| {
                                        tracing::error!(error = %e, "TcpAdapter stream write error");
                                    })
                                }
                            })
                            .await;
                        }
                        Ok(outcome) => {
                            write_frame(
                                &writer,
                                wire::frame_response(outcome).into_json_value(),
                                &id,
                            )
                            .await;
                        }
                    }
                };
                let handle = tasks.lock().await.spawn(task.instrument(span));
                if let Some(slot) = abort_slot {
                    *slot.lock().unwrap() = Some(handle);
                }
            }
            Err(e) => {
                tracing::error!(addr = %addr, error = %e, "TcpAdapter read error");
                peer_gone = true;
                break;
            }
        }
    }

    // The peer is gone: nothing this connection still owes can be delivered,
    // and a stream's producer learns only through the token. On shutdown the
    // in-flight tasks drain instead — serve()'s drain timeout is the backstop.
    if peer_gone {
        conn_inflight.cancel_all();
    }
}

/// Write one id-spliced reply frame, logging a failed write.
async fn write_frame(
    writer: &Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    mut frame: serde_json::Value,
    id: &str,
) {
    frame["id"] = serde_json::Value::String(id.to_string());
    let mut line = frame.to_string();
    line.push('\n');
    let mut w = writer.lock().await;
    if let Err(e) = w.write_all(line.as_bytes()).await {
        tracing::error!(error = %e, "TcpAdapter write error");
    }
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
