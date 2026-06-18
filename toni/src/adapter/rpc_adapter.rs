use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::rpc::{RpcCallInfo, RpcData, RpcError};

/// Callbacks the framework supplies to an RPC adapter.
///
/// The adapter calls `message` for every incoming message. Returning `Some(reply)`
/// means the caller is waiting for a response (request-response); returning `None`
/// means the message was fire-and-forget and the adapter should send nothing back.
pub struct RpcMessageCallbacks {
    on_message: Arc<
        dyn Fn(
                RpcData,
                RpcCallInfo,
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
            RpcCallInfo,
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
        context: RpcCallInfo,
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
    /// Called once before `into_lifecycle`. `patterns` is the full set of
    /// patterns this server handles — adapters that need to subscribe
    /// per-pattern (NATS, Redis) use this list; adapters that read a
    /// pattern field from the wire (TCP) can ignore it.
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()>;

    /// Consume the adapter and return a self-contained lifecycle handle.
    ///
    /// The implementation typically binds the listener, builds the
    /// receive-loop future, and packages a shutdown callback (captured
    /// from the adapter's own state) into the handle. The framework
    /// joins the serve future alongside every other adapter's serve.
    async fn into_lifecycle(
        self: Box<Self>,
    ) -> Result<crate::adapter::lifecycle_handles::RpcLifecycleHandle>;
}
