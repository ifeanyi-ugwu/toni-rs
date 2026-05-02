//! Internal lifecycle handles wrapping each adapter kind.
//!
//! Each handle owns its concrete adapter and exposes only the
//! [`ServerLifecycle`] surface to the orchestrator. Per-transport bind logic
//! happens in the handle's constructor — that's where the adapter's typed
//! methods are still in scope. Once stored as `Box<dyn ServerLifecycle>`
//! the orchestration layer cannot tell HTTP from gRPC.
//!
//! Adding a new adapter kind = add a new handle here + a typed
//! `use_*_adapter()` method on `ToniApplication`. Orchestration code in
//! `ToniApplication::bind`, `run`, and `close_adapters` does not change.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;

use crate::adapter::adapter_context::AdapterContext;
use crate::adapter::grpc_adapter::ErasedGrpcAdapter;
use crate::adapter::http_adapter::ErasedHttpAdapter;
use crate::adapter::rpc_adapter::{ErasedRpcAdapter, RpcMessageCallbacks};
use crate::adapter::server_lifecycle::ServerLifecycle;
use crate::adapter::websocket_adapter::{ErasedWebSocketAdapter, WsConnectionCallbacks};
use std::sync::Arc;

// ─── HTTP ────────────────────────────────────────────────────────────────────

pub(crate) struct HttpLifecycleHandle {
    adapter: Box<dyn ErasedHttpAdapter>,
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl HttpLifecycleHandle {
    pub(crate) async fn bind(
        mut adapter: Box<dyn ErasedHttpAdapter>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<Self> {
        let handle = adapter.listen(port, hostname, ctx).await?;
        Ok(Self {
            adapter,
            local_addr: handle.local_addr,
            serve: Some(handle.serve),
        })
    }
}

#[async_trait]
impl ServerLifecycle for HttpLifecycleHandle {
    fn name(&self) -> &'static str {
        "http"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.local_addr)
    }

    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        self.serve.take()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.adapter.close().await
    }
}

// ─── WebSocket (separate-port) ──────────────────────────────────────────────
//
// One handle per unique separate-port listener. A single `WebSocketAdapter`
// instance can produce N handles when N gateways were registered against
// distinct ports. The adapter itself is shared via
// `Arc<parking_lot::Mutex<Option<…>>>`. The first `shutdown()` call
// `Option::take`s the adapter out under the lock, releases the lock, then
// awaits close() on the owned value. Subsequent handles see `None` and
// no-op — this is how we get idempotent shutdown across siblings without
// ever holding a sync lock across `.await`.

pub(crate) type SharedWsAdapter = Arc<parking_lot::Mutex<Option<Box<dyn ErasedWebSocketAdapter>>>>;

pub(crate) struct WsLifecycleHandle {
    adapter: SharedWsAdapter,
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl WsLifecycleHandle {
    pub(crate) fn new(
        adapter: SharedWsAdapter,
        local_addr: SocketAddr,
        serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Self {
        Self {
            adapter,
            local_addr,
            serve: Some(serve),
        }
    }
}

#[async_trait]
impl ServerLifecycle for WsLifecycleHandle {
    fn name(&self) -> &'static str {
        "websocket"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.local_addr)
    }

    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        self.serve.take()
    }

    async fn shutdown(&mut self) -> Result<()> {
        let taken = self.adapter.lock().take();
        if let Some(mut adapter) = taken {
            adapter.close().await
        } else {
            Ok(())
        }
    }
}

// ─── RPC ─────────────────────────────────────────────────────────────────────

pub(crate) struct RpcLifecycleHandle {
    adapter: Box<dyn ErasedRpcAdapter>,
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl RpcLifecycleHandle {
    pub(crate) fn bind(
        mut adapter: Box<dyn ErasedRpcAdapter>,
        patterns: &[String],
        callbacks: Arc<RpcMessageCallbacks>,
    ) -> Result<Self> {
        adapter.bind(patterns, callbacks)?;
        let local_addr = adapter.local_addr();
        let serve = adapter.serve()?;
        Ok(Self {
            adapter,
            local_addr,
            serve: Some(serve),
        })
    }
}

#[async_trait]
impl ServerLifecycle for RpcLifecycleHandle {
    fn name(&self) -> &'static str {
        "rpc"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        self.serve.take()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.adapter.close().await
    }
}

// ─── gRPC ────────────────────────────────────────────────────────────────────

pub(crate) struct GrpcLifecycleHandle {
    adapter: Box<dyn ErasedGrpcAdapter>,
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl GrpcLifecycleHandle {
    pub(crate) fn bind(mut adapter: Box<dyn ErasedGrpcAdapter>) -> Result<Self> {
        adapter.bind()?;
        let local_addr = adapter.local_addr();
        let serve = adapter.serve()?;
        Ok(Self {
            adapter,
            local_addr,
            serve: Some(serve),
        })
    }
}

#[async_trait]
impl ServerLifecycle for GrpcLifecycleHandle {
    fn name(&self) -> &'static str {
        "grpc"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        self.serve.take()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.adapter.close().await
    }
}
