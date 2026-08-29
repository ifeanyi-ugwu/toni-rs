use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::OnceCell;
use toni::rpc::wire::{self, parse_response, ReplyFrame};
use toni::rpc::{ReplySink, RpcReplyStream};
use toni::{async_trait, RpcClientError, RpcClientTransport, RpcData};

use crate::IntoNatsServers;

/// NATS transport for [`RpcClient`].
///
/// Connections are established lazily on the first [`send`] or [`emit`] call so
/// the struct can be constructed synchronously inside a `provider_value!` or
/// `provider_factory!` block.
///
/// A streaming call (`open_stream`, ADR-0032) subscribes an explicit inbox and
/// feeds its frames to the caller's [`RpcReplyStream`] until the end marker.
/// The configured timeout bounds the gap to the next frame, the first
/// included. Dropping the reply stream before its end publishes a cancel
/// notice to `toni.rpc.cancel`, aborting the call on the server.
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

    async fn open_stream(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcReplyStream, RpcClientError> {
        let client = self.get_or_connect().await?.clone();

        // `send()` stays on the broker's native request; a stream needs an
        // inbox that outlives the first message, so it is explicit here.
        let inbox = client.new_inbox();
        let sub = client
            .subscribe(inbox.clone())
            .await
            .map_err(|e| RpcClientError::Transport(e.to_string()))?;

        let (cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (sink, stream) = RpcReplyStream::channel(32, move || {
            let _ = cancel_tx.send(());
        });
        tokio::spawn(forward_stream(
            sub,
            sink,
            cancel_rx,
            self.timeout,
            client.clone(),
            inbox.clone(),
        ));

        let subject = pattern.to_string();
        let payload = data_to_bytes(data);
        let result = if metadata.is_empty() {
            client.publish_with_reply(subject, inbox, payload).await
        } else {
            client
                .publish_with_reply_and_headers(subject, inbox, to_headers(metadata), payload)
                .await
        };
        result.map_err(|e| RpcClientError::Transport(e.to_string()))?;

        Ok(stream)
    }
}

/// Feed one streaming call's frames from its inbox subscription into the
/// caller's [`RpcReplyStream`], enforcing the per-frame gap deadline. A
/// dropped stream or an expired gap publishes the cancel notice so the server
/// stops producing.
async fn forward_stream(
    mut sub: async_nats::Subscriber,
    mut sink: ReplySink,
    mut cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    gap: Duration,
    client: async_nats::Client,
    inbox: String,
) {
    let cancel_notice = || Bytes::from(wire::frame_cancel(&inbox).to_string().into_bytes());
    loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                let _ = client.publish(crate::CANCEL_SUBJECT, cancel_notice()).await;
                break;
            }
            next = tokio::time::timeout(gap, sub.next()) => match next {
                Ok(Some(msg)) => match wire::parse_reply_frame(&msg.payload) {
                    ReplyFrame::Item(data) => {
                        let _ = sink.send(Ok(data)).await;
                    }
                    // A single-reply answer to a stream call: one item, then
                    // the end.
                    ReplyFrame::Single(result) => {
                        let _ = sink.send(result).await;
                        break;
                    }
                    ReplyFrame::End => break,
                    ReplyFrame::EndErr { message, status } => {
                        let _ = sink.send(Err(RpcClientError::Remote { message, status })).await;
                        break;
                    }
                },
                Ok(None) => {
                    let _ = sink
                        .send(Err(RpcClientError::Transport("connection closed".to_string())))
                        .await;
                    break;
                }
                Err(_) => {
                    let _ = sink.send(Err(RpcClientError::Timeout)).await;
                    let _ = client.publish(crate::CANCEL_SUBJECT, cancel_notice()).await;
                    break;
                }
            }
        }
    }
    let _ = sub.unsubscribe().await;
}

fn to_headers(metadata: HashMap<String, String>) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    for (key, value) in metadata {
        headers.insert(key.as_str(), value.as_str());
    }
    headers
}
