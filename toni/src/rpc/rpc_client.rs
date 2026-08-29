use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;

use crate::adapter::RpcClientTransport;
use crate::async_trait;
use crate::provider_scope::ProviderScope;
use crate::rpc::{RpcClientError, RpcData, RpcReplyStream};
use crate::traits_helpers::{Provider, ProviderContext};

/// Map a reply stream's items through `RpcData::parse`, keeping errors in
/// place.
fn parse_items<R>(stream: RpcReplyStream) -> BoxStream<'static, Result<R, RpcClientError>>
where
    R: serde::de::DeserializeOwned + Send + 'static,
{
    stream
        .map(|item| {
            item.and_then(|data| {
                data.parse::<R>()
                    .map_err(|e| RpcClientError::Transport(e.to_string()))
            })
        })
        .boxed()
}

/// Injectable handle for calling remote RPC services.
///
/// Wraps any [`RpcClientTransport`] and exposes two operations:
///
/// - [`send`] — request-response: dispatches a message and awaits the reply
/// - [`emit`] — fire-and-forget: dispatches a message and returns immediately
///
/// `RpcClient` is `Clone` (clones the inner `Arc`) so it can be shared freely
/// across providers injected from DI.
///
/// # Example
///
/// Register via `provide_factory!` with the `lifecycle` flag inside a module's
/// `providers` list:
///
/// ```ignore
/// provide_factory!("INVENTORY_CLIENT", |config: ConfigService| {
///     RpcClient::new(NatsClientTransport::new(config.get("NATS_URL")))
/// }, lifecycle)
/// ```
///
/// Inject into a service:
///
/// ```ignore
/// #[injectable]
/// pub struct InventoryService {
///     #[inject(token = "INVENTORY_CLIENT")] client: RpcClient,
/// }
/// impl InventoryService {
///     async fn notify_restock(&self, payload: serde_json::Value) -> Result<RpcData, RpcClientError> {
///         self.client.send("inventory.restock", RpcData::json(payload)).await
///     }
/// }
/// ```
///
/// [`send`]: RpcClient::send
/// [`emit`]: RpcClient::emit
#[derive(Clone)]
pub struct RpcClient {
    transport: Arc<dyn RpcClientTransport>,
}

impl RpcClient {
    pub fn new(transport: impl RpcClientTransport) -> Self {
        Self {
            transport: Arc::new(transport),
        }
    }

    /// Send a message and wait for the remote reply (request-response).
    ///
    /// Carries no headers. Use [`request`](Self::request) to attach per-call
    /// headers.
    pub async fn send(
        &self,
        pattern: impl AsRef<str>,
        data: RpcData,
    ) -> Result<RpcData, RpcClientError> {
        self.transport
            .send(pattern.as_ref(), data, HashMap::new())
            .await
    }

    /// Send a message without waiting for a reply (fire-and-forget).
    ///
    /// Carries no headers. Use [`request`](Self::request) to attach per-call
    /// headers.
    pub async fn emit(
        &self,
        pattern: impl AsRef<str>,
        data: RpcData,
    ) -> Result<(), RpcClientError> {
        self.transport
            .emit(pattern.as_ref(), data, HashMap::new())
            .await
    }

    /// Begin a request with per-call headers (auth tokens, trace ids, tenant,
    /// etc.). Chain [`header`](RpcRequest::header), then finish with
    /// [`send`](RpcRequest::send) / [`emit`](RpcRequest::emit) (or their typed
    /// `_json` variants).
    ///
    /// ```ignore
    /// client.request("inventory.restock")
    ///     .header("trace-id", trace_id)
    ///     .header("tenant", tenant)
    ///     .send(RpcData::json(payload))
    ///     .await?;
    /// ```
    pub fn request(&self, pattern: impl Into<String>) -> RpcRequest<'_> {
        RpcRequest {
            client: self,
            pattern: pattern.into(),
            headers: HashMap::new(),
        }
    }

    /// Typed request-response: serializes `data` to JSON, sends, and deserializes the reply.
    ///
    /// Shorthand for callers that work with concrete Rust types rather than raw `RpcData`.
    pub async fn send_json<T, R>(
        &self,
        pattern: impl AsRef<str>,
        data: &T,
    ) -> Result<R, RpcClientError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        let reply = self
            .transport
            .send(pattern.as_ref(), payload, HashMap::new())
            .await?;
        reply
            .parse::<R>()
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }

    /// Open a streaming call: one request, many replies (ADR-0032).
    ///
    /// Carries no headers — use [`request`](Self::request) to attach them.
    /// The reply stream yields items until the server's end marker; dropping
    /// it early sends the call's cancel notice, and the server's producer
    /// hears its cancellation token. Errors with
    /// [`StreamingUnsupported`](RpcClientError::StreamingUnsupported) on a
    /// transport predating the stream grammar.
    pub async fn stream(
        &self,
        pattern: impl AsRef<str>,
        data: RpcData,
    ) -> Result<RpcReplyStream, RpcClientError> {
        self.transport
            .open_stream(pattern.as_ref(), data, HashMap::new())
            .await
    }

    /// Typed streaming call: serializes `data`, opens the stream, and parses
    /// each item. An item that does not parse yields an `Err` in its place.
    pub async fn stream_json<T, R>(
        &self,
        pattern: impl AsRef<str>,
        data: &T,
    ) -> Result<BoxStream<'static, Result<R, RpcClientError>>, RpcClientError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned + Send + 'static,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        let stream = self
            .transport
            .open_stream(pattern.as_ref(), payload, HashMap::new())
            .await?;
        Ok(parse_items(stream))
    }

    /// Establish the connection to the remote service eagerly.
    ///
    /// Transports are lazy by default — they connect on the first `send` or `emit`.
    /// Call this explicitly (e.g. in an `#[on_application_bootstrap]` hook) when
    /// you want to surface connection failures at startup rather than on the first
    /// request.
    pub async fn connect(&self) -> Result<(), RpcClientError> {
        self.transport.connect().await
    }

    /// Gracefully close the connection to the remote service.
    ///
    /// Flushes any pending messages before closing. Call this in an
    /// `#[on_application_shutdown]` hook to ensure in-flight data is not lost
    /// before the process exits.
    pub async fn close(&self) -> Result<(), RpcClientError> {
        self.transport.close().await
    }

    /// Typed fire-and-forget: serializes `data` to JSON and emits without waiting for a reply.
    pub async fn emit_json<T>(
        &self,
        pattern: impl AsRef<str>,
        data: &T,
    ) -> Result<(), RpcClientError>
    where
        T: serde::Serialize,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        self.transport
            .emit(pattern.as_ref(), payload, HashMap::new())
            .await
    }
}

/// A pending RPC call accumulating per-call headers before dispatch.
///
/// Built by [`RpcClient::request`]; finish with [`send`](Self::send) /
/// [`emit`](Self::emit) or their typed `_json` variants.
pub struct RpcRequest<'a> {
    client: &'a RpcClient,
    pattern: String,
    headers: HashMap<String, String>,
}

impl RpcRequest<'_> {
    /// Attach one header. Chainable; a repeated key overwrites.
    ///
    /// The transport carries these as whatever it calls headers — NATS headers, AMQP headers, Kafka
    /// record headers, MQTT user properties — and the handler reads them back through
    /// [`RpcContext::headers`](crate::context::RpcContext::headers).
    #[doc(alias = "metadata")]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Dispatch and await the remote reply (request-response).
    pub async fn send(self, data: RpcData) -> Result<RpcData, RpcClientError> {
        self.client
            .transport
            .send(&self.pattern, data, self.headers)
            .await
    }

    /// Dispatch without waiting for a reply (fire-and-forget).
    pub async fn emit(self, data: RpcData) -> Result<(), RpcClientError> {
        self.client
            .transport
            .emit(&self.pattern, data, self.headers)
            .await
    }

    /// Typed request-response: serializes `data`, sends, deserializes the reply.
    pub async fn send_json<T, R>(self, data: &T) -> Result<R, RpcClientError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        let reply = self
            .client
            .transport
            .send(&self.pattern, payload, self.headers)
            .await?;
        reply
            .parse::<R>()
            .map_err(|e| RpcClientError::Transport(e.to_string()))
    }

    /// Typed fire-and-forget: serializes `data` and emits.
    pub async fn emit_json<T>(self, data: &T) -> Result<(), RpcClientError>
    where
        T: serde::Serialize,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        self.client
            .transport
            .emit(&self.pattern, payload, self.headers)
            .await
    }

    /// Open a streaming call carrying the accumulated headers
    /// (ADR-0032).
    pub async fn stream(self, data: RpcData) -> Result<RpcReplyStream, RpcClientError> {
        self.client
            .transport
            .open_stream(&self.pattern, data, self.headers)
            .await
    }

    /// Typed streaming call: serializes `data`, opens the stream, and parses
    /// each item.
    pub async fn stream_json<T, R>(
        self,
        data: &T,
    ) -> Result<BoxStream<'static, Result<R, RpcClientError>>, RpcClientError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned + Send + 'static,
    {
        let payload =
            RpcData::from_serialize(data).map_err(|e| RpcClientError::Transport(e.to_string()))?;
        let stream = self
            .client
            .transport
            .open_stream(&self.pattern, payload, self.headers)
            .await?;
        Ok(parse_items(stream))
    }
}

#[async_trait]
impl Provider for RpcClient {
    fn get_token(&self) -> String {
        crate::di::token_of::<Self>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.clone())
    }

    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }

    async fn on_application_bootstrap(&self) -> crate::InitResult {
        self.connect()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn on_application_shutdown(&self, _signal: Option<String>) {
        if let Err(e) = self.close().await {
            tracing::error!(error = %e, "RpcClient close failed at shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SingleOnly;

    #[async_trait]
    impl RpcClientTransport for SingleOnly {
        async fn send(
            &self,
            _pattern: &str,
            data: RpcData,
            _metadata: HashMap<String, String>,
        ) -> Result<RpcData, RpcClientError> {
            Ok(data)
        }

        async fn emit(
            &self,
            _pattern: &str,
            _data: RpcData,
            _metadata: HashMap<String, String>,
        ) -> Result<(), RpcClientError> {
            Ok(())
        }
    }

    #[test]
    fn a_transport_without_the_grammar_refuses_a_stream_call() {
        let client = RpcClient::new(SingleOnly);
        let result = futures_executor::block_on(client.stream("p", RpcData::text("")));
        assert!(matches!(result, Err(RpcClientError::StreamingUnsupported)));
    }
}
