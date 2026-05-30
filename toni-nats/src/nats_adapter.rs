use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use toni::{RpcAdapter, RpcCallInfo, RpcData, RpcError, RpcMessageCallbacks};

use crate::IntoNatsServers;

/// NATS transport adapter for the Toni RPC gateway.
///
/// Subscribes once per pattern. Each NATS subject maps directly to a handler
/// pattern — no envelope wrapper needed since the subject IS the routing key.
///
/// **Request-response**: set a NATS reply-to inbox on the outbound message. The
/// adapter publishes the response there.
///
/// **Fire-and-forget**: omit the reply-to inbox. If a reply-to is set but the
/// handler is an `#[event_pattern]`, the adapter publishes a null ack so the
/// caller's pending request can close rather than timing out.
///
/// **Payload format** (inbound): raw JSON bytes.
/// **Response format** (outbound): `{"response":<json>}` or `{"err":{"message":"...","status":"..."}}`
///
/// # Example
///
/// ```rust,no_run
/// app.use_rpc_adapter(toni_nats::NatsAdapter::new("nats://localhost:4222")).unwrap();
/// ```
///
/// Test with the NATS CLI:
///
/// ```bash
/// # request-response
/// nats req order.create '{"item":"keyboard","qty":3}'
///
/// # fire-and-forget publish
/// nats pub order.shipped '{"order_id":1001}'
/// ```
pub struct NatsAdapter {
    servers: Vec<String>,
    patterns: Vec<String>,
    callbacks: Option<Arc<RpcMessageCallbacks>>,
}

impl NatsAdapter {
    pub fn new(servers: impl IntoNatsServers) -> Self {
        Self {
            servers: servers.into_servers(),
            patterns: Vec::new(),
            callbacks: None,
        }
    }
}

#[toni::async_trait]
impl RpcAdapter for NatsAdapter {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
        self.patterns = patterns.to_vec();
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::RpcLifecycleHandle> {
        let servers = self.servers.clone();
        let patterns = std::mem::take(&mut self.patterns);
        let callbacks = self
            .callbacks
            .take()
            .expect("bind() must be called before into_lifecycle()");

        let serve = Box::pin(async move {
            let servers_for_log = servers.join(", ");
            // Retry until the server is reachable so a slow-starting NATS
            // container doesn't kill the whole process on startup.
            // event_callback fires on the real TCP handshake, not when connect() returns.
            let client = async_nats::ConnectOptions::new()
                .retry_on_initial_connect()
                .event_callback(move |event| {
                    let servers = servers_for_log.clone();
                    async move {
                        match event {
                            async_nats::Event::Connected => {
                                tracing::info!(servers, "NatsAdapter connected")
                            }
                            async_nats::Event::Disconnected => {
                                tracing::warn!(servers, "NatsAdapter disconnected")
                            }
                            async_nats::Event::ServerError(e) => {
                                tracing::error!(error = %e, "NatsAdapter server error")
                            }
                            async_nats::Event::ClientError(e) => {
                                tracing::error!(error = %e, "NatsAdapter client error")
                            }
                            _ => {}
                        }
                    }
                })
                .connect(servers.clone())
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[NatsAdapter] Failed to connect to {} — {}",
                        servers.join(", "),
                        e
                    )
                });

            let mut handles = Vec::new();

            for pattern in patterns {
                let client = client.clone();
                let callbacks = callbacks.clone();

                let mut subscriber = client.subscribe(pattern.clone()).await.unwrap_or_else(|e| {
                    panic!("[NatsAdapter] Failed to subscribe to {} — {}", pattern, e)
                });

                tracing::info!(pattern, "NatsAdapter subscribed");

                handles.push(tokio::spawn(async move {
                    while let Some(msg) = subscriber.next().await {
                        let client = client.clone();
                        let callbacks = callbacks.clone();
                        let subject = msg.subject.to_string();
                        let reply_to = msg.reply.clone();
                        let payload = msg.payload.clone();
                        let headers = msg.headers.clone();

                        tokio::spawn(async move {
                            let data = match serde_json::from_slice::<serde_json::Value>(&payload) {
                                Ok(v) => RpcData::Json(v),
                                Err(_) => RpcData::Binary(payload.to_vec()),
                            };

                            let mut ctx = RpcCallInfo::new(subject);
                            if let Some(headers) = headers {
                                for (name, values) in headers.iter() {
                                    if let Some(first) = values.iter().next() {
                                        ctx.metadata
                                            .insert(name.to_string(), first.to_string());
                                    }
                                }
                            }
                            let outcome =
                                std::panic::AssertUnwindSafe(callbacks.message(data, ctx))
                                    .catch_unwind()
                                    .await;

                            let Some(inbox) = reply_to else {
                                if outcome.is_err() {
                                    tracing::error!(
                                        "RPC handler panicked on fire-and-forget message"
                                    );
                                }
                                return;
                            };

                            let response_bytes = match outcome {
                                Err(_) => {
                                    tracing::error!(
                                        "RPC handler panicked; returning error to caller"
                                    );
                                    Bytes::from(
                                        serde_json::json!({
                                            "err": { "message": "internal server error", "status": "error" }
                                        })
                                        .to_string(),
                                    )
                                }
                                Ok(outcome) => match outcome {
                                    Ok(Some(reply_data)) => match reply_data {
                                        RpcData::Binary(b) => Bytes::from(b),
                                        RpcData::Json(v) => Bytes::from(
                                            serde_json::json!({ "response": v }).to_string(),
                                        ),
                                        RpcData::Text(s) => Bytes::from(
                                            serde_json::json!({ "response": s }).to_string(),
                                        ),
                                    },
                                    Ok(None) => {
                                        // #[event_pattern] handler but caller set a reply-to — send ack.
                                        Bytes::from(
                                            serde_json::json!({ "response": null }).to_string(),
                                        )
                                    }
                                    Err(RpcError::AppError(arc)) => {
                                        let envelope = toni::rpc::RpcError::AppError(arc).to_data();
                                        match envelope {
                                            RpcData::Binary(b) => Bytes::from(b),
                                            RpcData::Json(v) => Bytes::from(
                                                serde_json::json!({ "response": v }).to_string(),
                                            ),
                                            RpcData::Text(s) => Bytes::from(
                                                serde_json::json!({ "response": s }).to_string(),
                                            ),
                                        }
                                    }
                                    Err(e) => {
                                        let status = error_status(&e);
                                        Bytes::from(
                                            serde_json::json!({
                                                "err": { "message": e.to_string(), "status": status }
                                            })
                                            .to_string(),
                                        )
                                    }
                                },
                            };

                            if let Err(e) = client.publish(inbox, response_bytes).await {
                                tracing::error!(error = %e, "NatsAdapter publish error");
                            }
                        });
                    }
                }));
            }

            futures::future::join_all(handles).await;
        });

        // NATS has no listener — no local_addr — and no graceful shutdown
        // signal in the current implementation; the close callback is a
        // no-op.
        Ok(toni::RpcLifecycleHandle::new(None, serve, || async { Ok(()) }))
    }
}

fn error_status(e: &RpcError) -> &'static str {
    match e {
        RpcError::PatternNotFound(_) => "not_found",
        RpcError::Forbidden(_) => "forbidden",
        RpcError::Internal(_) => "error",
        RpcError::AppError(_) => unreachable!(
            "RpcError::AppError is routed to the Ok+envelope frame before \
             reaching wire-Err framing"
        ),
    }
}
