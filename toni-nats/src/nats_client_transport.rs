use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::OnceCell;
use toni::rpc::wire::parse_response;
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::IntoNatsServers;

/// NATS transport for [`RpcClient`].
///
/// Connections are established lazily on the first [`send`] or [`emit`] call so
/// the struct can be constructed synchronously inside a `provider_value!` or
/// `provider_factory!` block.
///
/// # Example
///
/// ```ignore
/// provider_value!(
///     "INVENTORY_CLIENT",
///     toni::RpcClient::new(toni_nats::NatsClientTransport::new("nats://localhost:4222"))
/// )
/// ```
///
/// [`RpcClient`]: toni::RpcClient
/// [`send`]: NatsClientTransport::send
/// [`emit`]: NatsClientTransport::emit
pub struct NatsClientTransport {
    servers: Vec<String>,
    timeout: Duration,
    client: OnceCell<async_nats::Client>,
}

impl NatsClientTransport {
    pub fn new(servers: impl IntoNatsServers) -> Self {
        Self {
            servers: servers.into_servers(),
            timeout: Duration::from_secs(5),
            client: OnceCell::new(),
        }
    }

    /// Override the request-response timeout (default: 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn get_or_connect(&self) -> Result<&async_nats::Client, RpcClientError> {
        let servers = self.servers.clone();
        self.client
            .get_or_try_init(|| async move {
                async_nats::connect(servers)
                    .await
                    .map_err(|e| RpcClientError::Transport(e.to_string()))
            })
            .await
    }
}

fn data_to_bytes(data: RpcData) -> Bytes {
    match data {
        RpcData::Json(v) => Bytes::from(v.to_string()),
        RpcData::Text(s) => Bytes::from(s.into_bytes()),
        RpcData::Binary(b) => Bytes::from(b),
    }
}

#[async_trait]
impl RpcClientTransport for NatsClientTransport {
    async fn connect(&self) -> Result<(), RpcClientError> {
        self.get_or_connect().await?;
        Ok(())
    }

    async fn close(&self) -> Result<(), RpcClientError> {
        if let Some(client) = self.client.get() {
            client
                .flush()
                .await
                .map_err(|e| RpcClientError::Transport(e.to_string()))?;
        }
        Ok(())
    }

    async fn send(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcData, RpcClientError> {
        let client = self.get_or_connect().await?;
        let subject = pattern.to_string();
        let payload = data_to_bytes(data);

        let request = if metadata.is_empty() {
            tokio::time::timeout(self.timeout, client.request(subject, payload)).await
        } else {
            tokio::time::timeout(
                self.timeout,
                client.request_with_headers(subject, to_headers(metadata), payload),
            )
            .await
        };
        let response = request
            .map_err(|_| RpcClientError::Timeout)?
            .map_err(|e| RpcClientError::Transport(e.to_string()))?;

        parse_response(&response.payload)
    }

    async fn emit(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<(), RpcClientError> {
        let client = self.get_or_connect().await?;
        let subject = pattern.to_string();
        let payload = data_to_bytes(data);

        let result = if metadata.is_empty() {
            client.publish(subject, payload).await
        } else {
            client
                .publish_with_headers(subject, to_headers(metadata), payload)
                .await
        };
        result.map_err(|e| RpcClientError::Transport(e.to_string()))
    }
}

fn to_headers(metadata: HashMap<String, String>) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    for (key, value) in metadata {
        headers.insert(key.as_str(), value.as_str());
    }
    headers
}
