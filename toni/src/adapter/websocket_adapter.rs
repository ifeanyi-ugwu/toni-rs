use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::adapter::server_handle::ServerHandle;
use crate::http_helpers::RequestPart;
use crate::websocket::{WsError, WsMessage, WsSink};

/// Result of the message callback — tells the adapter what to do next.
pub enum MessageCallbackResult {
    /// Keep reading; nothing to stream.
    Continue,
    /// Close the read loop.
    Stop,
    /// Spawn a task that drives this stream and forwards items to the client.
    /// The adapter aborts the task when the connection closes.
    Stream(BoxStream<'static, WsMessage>),
}

/// Callbacks the framework supplies to an adapter for one gateway path.
///
/// The adapter calls these at the right moment in the connection lifecycle — it never
/// touches `GatewayWrapper`, `WsGatewayHandle`, or `ConnectionManager` directly.
pub struct WsConnectionCallbacks {
    on_connect: Arc<
        dyn Fn(
                RequestPart,
                Arc<dyn WsSink>,
            ) -> Pin<Box<dyn Future<Output = Result<String, WsError>> + Send>>
            + Send
            + Sync,
    >,
    on_message: Arc<
        dyn Fn(String, WsMessage) -> Pin<Box<dyn Future<Output = MessageCallbackResult> + Send>>
            + Send
            + Sync,
    >,
    on_disconnect: Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
}

impl WsConnectionCallbacks {
    pub(crate) fn new(
        on_connect: impl Fn(
            RequestPart,
            Arc<dyn WsSink>,
        ) -> Pin<Box<dyn Future<Output = Result<String, WsError>> + Send>>
        + Send
        + Sync
        + 'static,
        on_message: impl Fn(
            String,
            WsMessage,
        ) -> Pin<Box<dyn Future<Output = MessageCallbackResult> + Send>>
        + Send
        + Sync
        + 'static,
        on_disconnect: impl Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            on_connect: Arc::new(on_connect),
            on_message: Arc::new(on_message),
            on_disconnect: Arc::new(on_disconnect),
        }
    }

    /// Called by the adapter when a new client connects.
    ///
    /// Pass the upgrade request parts and an adapter-owned sender for this connection.
    /// Returns the assigned client id, or an error if a guard rejects the connection.
    pub async fn connect(
        &self,
        parts: RequestPart,
        sender: Arc<dyn WsSink>,
    ) -> Result<String, WsError> {
        (self.on_connect)(parts, sender).await
    }

    /// Called by the adapter for each decoded message from a connected client.
    pub async fn message(&self, client_id: String, msg: WsMessage) -> MessageCallbackResult {
        (self.on_message)(client_id, msg).await
    }

    /// Called by the adapter when the read loop ends (client disconnected).
    pub async fn disconnect(&self, client_id: String) {
        (self.on_disconnect)(client_id).await
    }
}

/// Interface for standalone (separate-port) WebSocket server adapters.
///
/// Implement `bind`, `listen`, and `close`. The framework constructs
/// [`WsConnectionCallbacks`] with all lifecycle logic embedded — the adapter never
/// touches `GatewayWrapper` or `ConnectionManager` directly.
///
/// Same-port (HTTP upgrade) gateways are handled by [`HttpAdapter::bind_ws`].
#[async_trait]
pub trait WebSocketAdapter: Send + Sync + 'static {
    /// Register a gateway path for `port`, storing `callbacks` for each incoming connection.
    ///
    /// Called once per gateway before `listen` is called for the same port.
    /// **Default:** returns error — implement for separate-port support.
    fn bind(&mut self, port: u16, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        let _ = (port, path, callbacks);
        Err(anyhow::anyhow!(
            "This WebSocket adapter does not support separate-port servers"
        ))
    }

    /// Bind the listening socket for `port` and return a handle to the running server.
    ///
    /// Called once per unique port after all `bind` calls for that port. The returned
    /// future resolves once the socket is bound — `handle.local_addr` reflects the
    /// actual bound address. Awaiting `handle.serve` runs the accept loop, which the
    /// framework joins alongside the HTTP server future.
    ///
    /// **Default:** returns a future that immediately errors — implement for separate-port support.
    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>> {
        let _ = (port, hostname);
        Box::pin(async {
            Err(anyhow::anyhow!(
                "This WebSocket adapter does not support separate-port servers"
            ))
        })
    }

    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

