use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{FutureExt, SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::http_helpers::RequestPart;
use toni::websocket::{SendError, TrySendError, WsMessage, WsSink};
use toni::{
    MessageCallbackResult, WebSocketAdapter, WsConnectionCallbacks, WsLifecycleHandle,
};

// ── TokioSender ───────────────────────────────────────────────────────────────

struct TokioSender {
    inner: mpsc::Sender<WsMessage>,
}

impl TokioSender {
    fn new(tx: mpsc::Sender<WsMessage>) -> Self {
        Self { inner: tx }
    }
}

#[async_trait]
impl WsSink for TokioSender {
    async fn send(&self, message: WsMessage) -> Result<(), SendError> {
        self.inner.send(message).await.map_err(|_| SendError)
    }

    fn try_send(&self, message: WsMessage) -> Result<(), TrySendError> {
        self.inner.try_send(message).map_err(|e| match e {
            mpsc::error::TrySendError::Full(msg) => TrySendError::Full(msg),
            mpsc::error::TrySendError::Closed(_) => TrySendError::Closed,
        })
    }
}

// ── TungsteniteAdapter ────────────────────────────────────────────────────────

struct PortEntry {
    // path → callbacks; raw TCP has no path info, so we use the first registered binding
    bindings: HashMap<String, Arc<WsConnectionCallbacks>>,
}

impl PortEntry {
    fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }
}

pub struct TungsteniteAdapter {
    ports: HashMap<u16, PortEntry>,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl TungsteniteAdapter {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            ports: HashMap::new(),
            shutdown_tx: Arc::new(tx),
        }
    }
}

impl Default for TungsteniteAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WebSocketAdapter for TungsteniteAdapter {
    fn bind(&mut self, port: u16, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        self.ports
            .entry(port)
            .or_insert_with(PortEntry::new)
            .bindings
            .insert(path.to_string(), callbacks);
        Ok(())
    }

    async fn into_lifecycle_handles(
        mut self: Box<Self>,
        ports: Vec<(u16, String)>,
    ) -> Result<Vec<WsLifecycleHandle>> {
        let mut handles = Vec::with_capacity(ports.len());
        for (port, hostname) in ports {
            let entry = match self.ports.remove(&port) {
                Some(e) => e,
                None => continue,
            };
            // Raw TCP has no path info — use the first registered callbacks.
            let bindings: Vec<Arc<WsConnectionCallbacks>> = entry.bindings.into_values().collect();
            let addr = format!("{}:{}", hostname, port);
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let shutdown_tx = self.shutdown_tx.clone();

            let listener = TcpListener::bind(&addr).await
                .map_err(|e| anyhow::anyhow!("Failed to bind WebSocket port {}: {}", addr, e))?;
            let local_addr = listener.local_addr()
                .map_err(|e| anyhow::anyhow!("Failed to get local address: {}", e))?;

            let serve = Box::pin(async move {
                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            let (stream, _) = match result {
                                Ok(r) => r,
                                Err(e) => { tracing::error!(error = %e, "WebSocket accept error"); continue; }
                            };
                            if let Some(callbacks) = bindings.first().cloned() {
                                tokio::spawn(async move {
                                    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                                        Ok(ws) => ws,
                                        Err(e) => { tracing::error!(error = %e, "WebSocket handshake error"); return; }
                                    };
                                    run_ws_connection(ws_stream, callbacks).await;
                                });
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() { break; }
                        }
                    }
                }
            });

            handles.push(WsLifecycleHandle::new(local_addr, serve, move || async move {
                let _ = shutdown_tx.send(true);
                Ok(())
            }));
        }
        Ok(handles)
    }
}

// ── Shared connection loop ────────────────────────────────────────────────────

async fn run_ws_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    callbacks: Arc<WsConnectionCallbacks>,
) {
    let (write, read) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            if let Ok(m) = ws_message_to_tungstenite(msg) {
                if write.send(m).await.is_err() {
                    break;
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

    // Raw TCP has no HTTP upgrade request; pass synthetic empty parts.
    let parts: RequestPart = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri("/")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    let client_id = match callbacks.connect(parts, sender.clone()).await {
        Ok(id) => id,
        Err(_) => return,
    };

    let stream_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let stream_tasks_inner = stream_tasks.clone();

    let mut read = read;
    let panicked = std::panic::AssertUnwindSafe(async {
        while let Some(result) = read.next().await {
            match result {
                Ok(Message::Text(t)) => {
                    match callbacks
                        .message(client_id.clone(), WsMessage::Text(t.to_string()))
                        .await
                    {
                        MessageCallbackResult::Continue => {}
                        MessageCallbackResult::Stop => break,
                        MessageCallbackResult::Stream(stream) => {
                            let sink = sender.clone();
                            let handle = tokio::spawn(async move {
                                use futures_util::StreamExt;
                                tokio::pin!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = sink.send(msg).await;
                                }
                            });
                            stream_tasks_inner.lock().unwrap().push(handle);
                        }
                    }
                }
                Ok(Message::Binary(b)) => {
                    match callbacks
                        .message(client_id.clone(), WsMessage::Binary(b.to_vec()))
                        .await
                    {
                        MessageCallbackResult::Continue => {}
                        MessageCallbackResult::Stop => break,
                        MessageCallbackResult::Stream(stream) => {
                            let sink = sender.clone();
                            let handle = tokio::spawn(async move {
                                use futures_util::StreamExt;
                                tokio::pin!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = sink.send(msg).await;
                                }
                            });
                            stream_tasks_inner.lock().unwrap().push(handle);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            }
        }
    })
    .catch_unwind()
    .await
    .is_err();

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    if panicked {
        tracing::error!(client_id = %client_id, "WebSocket handler panicked; closing connection");
    }
    callbacks.disconnect(client_id).await;
}

fn ws_message_to_tungstenite(msg: WsMessage) -> Result<Message> {
    match msg {
        WsMessage::Text(t) => Ok(Message::Text(t.into())),
        WsMessage::Binary(b) => Ok(Message::Binary(b.into())),
        WsMessage::Ping(d) => Ok(Message::Ping(d.into())),
        WsMessage::Pong(d) => Ok(Message::Pong(d.into())),
        WsMessage::Close => Ok(Message::Close(None)),
    }
}
