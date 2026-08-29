use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use toni::rpc::wire;
use toni::{RpcAdapter, RpcCallInfo, RpcData, RpcMessageCallbacks};

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
/// **Streaming** (ADR-0032): a stream answer publishes item frames
/// (`{"stream":…}`, `{"stream_b64":…}` for `Binary`) to the reply inbox,
/// closed by `{"end":true}` or `{"end":true,"err":{…}}`. A caller abandons an
/// in-flight call by publishing `{"cancel":true,"key":"<inbox>"}` to
/// `toni.rpc.cancel`; every instance sees the notice and the one holding the
/// call aborts it, firing the execution's cancellation token.
///
/// # Example
///
/// ```ignore
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
    fn register_handlers(
        &mut self,
        patterns: &[String],
        callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<()> {
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
            .expect("register_handlers() must be called before into_lifecycle()");

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

            // Streaming calls in flight, keyed by reply inbox, abortable by a
            // cancel notice.
            let inflight_calls = wire::Inflight::new();
            {
                let mut cancel_sub = client
                    .subscribe(crate::CANCEL_SUBJECT)
                    .await
                    .unwrap_or_else(|e| {
                        panic!(
                            "[NatsAdapter] Failed to subscribe to {} — {}",
                            crate::CANCEL_SUBJECT,
                            e
                        )
                    });
                let inflight_calls = inflight_calls.clone();
                handles.push(tokio::spawn(async move {
                    while let Some(msg) = cancel_sub.next().await {
                        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&msg.payload)
                        else {
                            continue;
                        };
                        if v.get("cancel").and_then(|c| c.as_bool()) == Some(true) {
                            if let Some(key) = v["key"].as_str() {
                                inflight_calls.cancel(key);
                            }
                        }
                    }
                }));
            }

            for pattern in patterns {
                let client = client.clone();
                let callbacks = callbacks.clone();
                let inflight_calls = inflight_calls.clone();

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

                        // Register a request-shaped call before dispatch so a
                        // cancel notice can abort it mid-handler or mid-drain.
                        // A notice racing this registration is dropped — the
                        // cancel channel is best-effort on a broker.
                        let (abort_slot, guard) = match &reply_to {
                            Some(inbox) => {
                                let abort_slot = Arc::new(std::sync::Mutex::new(
                                    None::<tokio::task::AbortHandle>,
                                ));
                                let slot = abort_slot.clone();
                                let guard = inflight_calls.register(inbox.to_string(), move || {
                                    if let Some(handle) = slot.lock().unwrap().take() {
                                        handle.abort();
                                    }
                                });
                                (Some(abort_slot), Some(guard))
                            }
                            None => (None, None),
                        };

                        let handle = tokio::spawn(async move {
                            let _guard = guard;
                            let data = match serde_json::from_slice::<serde_json::Value>(&payload) {
                                Ok(v) => RpcData::Json(v),
                                Err(_) => RpcData::Binary(payload.to_vec()),
                            };

                            let mut ctx = RpcCallInfo::new(subject);
                            if let Some(headers) = headers {
                                for (name, values) in headers.iter() {
                                    if let Some(first) = values.iter().next() {
                                        ctx.headers.insert(name.to_string(), first.to_string());
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
                                    Bytes::from(wire::frame_panic().into_bytes())
                                }
                                Ok(Ok(toni::RpcHandlerOutput::Stream(stream))) => {
                                    wire::drive_reply_stream(stream, |frame| {
                                        let client = client.clone();
                                        let inbox = inbox.clone();
                                        async move {
                                            client
                                                .publish(
                                                    inbox,
                                                    Bytes::from(frame.to_string().into_bytes()),
                                                )
                                                .await
                                                .map_err(|e| {
                                                    tracing::error!(
                                                        error = %e,
                                                        "NatsAdapter stream publish error"
                                                    );
                                                })
                                        }
                                    })
                                    .await;
                                    return;
                                }
                                Ok(outcome) => {
                                    Bytes::from(wire::frame_response(outcome).into_bytes())
                                }
                            };

                            if let Err(e) = client.publish(inbox, response_bytes).await {
                                tracing::error!(error = %e, "NatsAdapter publish error");
                            }
                        });
                        if let Some(slot) = abort_slot {
                            *slot.lock().unwrap() = Some(handle.abort_handle());
                        }
                    }
                }));
            }

            futures::future::join_all(handles).await;
        });

        // NATS has no listener — no local_addr — and no graceful shutdown
        // signal in the current implementation; the close callback is a
        // no-op.
        Ok(toni::RpcLifecycleHandle::new(None, serve, || async {
            Ok(())
        }))
    }
}
