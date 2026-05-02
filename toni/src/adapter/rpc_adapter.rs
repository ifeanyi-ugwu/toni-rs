use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::rpc::{RpcContext, RpcData, RpcError};

/// Callbacks the framework supplies to an RPC adapter.
///
/// The adapter calls `message` for every incoming message. Returning `Some(reply)`
/// means the caller is waiting for a response (request-response); returning `None`
/// means the message was fire-and-forget and the adapter should send nothing back.
pub struct RpcMessageCallbacks {
    on_message: Arc<
        dyn Fn(
                RpcData,
                RpcContext,
            )
                -> Pin<Box<dyn Future<Output = Result<Option<RpcData>, RpcError>> + Send>>
            + Send
            + Sync,
    >,
}

impl RpcMessageCallbacks {
    pub(crate) fn new(
        on_message: impl Fn(
            RpcData,
            RpcContext,
        )
            -> Pin<Box<dyn Future<Output = Result<Option<RpcData>, RpcError>> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            on_message: Arc::new(on_message),
        }
    }

    /// Called by the adapter for each decoded incoming message.
    ///
    /// - `Ok(Some(reply))` — send this reply (request-response)
    /// - `Ok(None)` — fire-and-forget, send nothing
    /// - `Err(e)` — handler or framework error; adapter decides how to serialize it
    pub async fn message(
        &self,
        data: RpcData,
        context: RpcContext,
    ) -> Result<Option<RpcData>, RpcError> {
        (self.on_message)(data, context).await
    }
}

/// Interface for RPC transport adapters.
///
/// Implement `bind`, `serve`, and optionally `close`. The framework constructs
/// [`RpcMessageCallbacks`] with all dispatch logic embedded — the adapter never
/// touches handler types directly.
///
/// `patterns` in `bind` tells subscription-based adapters (NATS, Redis, Kafka)
/// which subjects to subscribe to. Envelope-based adapters (TCP) can ignore it
/// and route by the pattern field in the message.
#[async_trait]
pub trait RpcAdapter: Send + Sync + 'static {
    /// Register message handlers for this transport.
    ///
    /// Called once before `serve`. `patterns` is the full set of patterns this
    /// server handles — adapters that need to subscribe per-pattern (NATS, Redis)
    /// use this list; adapters that read a pattern field from the wire (TCP) can ignore it.
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()>;

    /// Seal configuration and return the running receive loop future.
    ///
    /// Called once after `bind`. The returned future is the accept/receive loop —
    /// the framework joins it alongside the HTTP server future so no top-level
    /// spawn is needed in the adapter.
    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    /// Local listening address, if this adapter binds to one.
    ///
    /// Listener-based transports (TCP, UDP, gRPC) return `Some` after `bind` —
    /// useful when port 0 was passed and the OS assigned a port. Subject-based
    /// transports (NATS, Kafka, MQTT) return `None`.
    fn local_addr(&self) -> Option<SocketAddr> {
        None
    }

    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Object-safe internal facade over [`RpcAdapter`] for storage in `ToniApplication`.
#[async_trait]
pub(crate) trait ErasedRpcAdapter: Send + Sync + 'static {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()>;
    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;
    fn local_addr(&self) -> Option<SocketAddr>;
    async fn close(&mut self) -> Result<()>;
}

#[async_trait]
impl<R: RpcAdapter> ErasedRpcAdapter for R {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()> {
        <R as RpcAdapter>::bind(self, patterns, callbacks)
    }

    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        <R as RpcAdapter>::serve(self)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        <R as RpcAdapter>::local_addr(self)
    }

    async fn close(&mut self) -> Result<()> {
        <R as RpcAdapter>::close(self).await
    }
}
