use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures_util::FutureExt;
use tokio::net::UdpSocket;
use toni::{RpcAdapter, RpcContext, RpcData, RpcError, RpcMessageCallbacks};

/// Maximum UDP datagram payload (theoretical max minus IPv4 + UDP headers).
const MAX_DATAGRAM: usize = 65_507;

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
/// - **No graceful shutdown** in v1: the receive loop runs until the process
///   exits.
pub struct UdpAdapter {
    host: String,
    port: u16,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl UdpAdapter {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            callbacks: None,
        }
    }
}

impl RpcAdapter for UdpAdapter {
    fn bind(&mut self, _patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
        self.callbacks = Some(callbacks);
        Ok(())
    }

    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        let host = self.host.clone();
        let port = self.port;
        let callbacks = self
            .callbacks
            .take()
            .expect("bind() must be called before serve()");

        Ok(Box::pin(async move {
            let addr = format!("{}:{}", host, port);
            let socket = UdpSocket::bind(&addr)
                .await
                .unwrap_or_else(|e| panic!("UdpAdapter: failed to bind {} — {}", addr, e));
            let socket = Arc::new(socket);

            tracing::info!(addr, "UdpAdapter listening");

            let mut buf = vec![0u8; MAX_DATAGRAM];
            loop {
                match socket.recv_from(&mut buf).await {
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

                        tokio::spawn(async move {
                            handle_datagram(socket, src, payload, callbacks).await;
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "UdpAdapter recv error");
                    }
                }
            }
        }))
    }
}

fn error_status(e: &RpcError) -> &'static str {
    match e {
        RpcError::PatternNotFound(_) => "not_found",
        RpcError::Forbidden(_) => "forbidden",
        RpcError::Internal(_) => "error",
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
    let ctx = RpcContext::new(pattern);

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
                // Handler is fire-and-forget (#[event_pattern]) but caller
                // sent an id — send an explicit ack so the caller can close
                // the pending request rather than timing out.
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
