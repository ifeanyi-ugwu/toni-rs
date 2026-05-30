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

use crate::adapter::grpc_adapter::GrpcAdapter;
use crate::adapter::grpc_service_trait::{GrpcServiceTrait, ResolvedGrpcEnhancers};
use crate::adapter::rpc_adapter::{RpcAdapter, RpcMessageCallbacks};
use crate::adapter::server_lifecycle::ServerLifecycle;
use crate::adapter::websocket_adapter::{WebSocketAdapter, WsConnectionCallbacks};
use std::sync::Arc;

// ─── HTTP ────────────────────────────────────────────────────────────────────

/// Boxed shutdown action the adapter produces alongside the serve future.
/// Lets the lifecycle handle drive shutdown without holding a reference
/// back to the adapter — the adapter's own state (channel sender, signal,
/// etc.) is captured in the closure and the handle just calls it.
pub type ShutdownCallback =
    Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>> + Send + Sync>;

/// Lifecycle handle for an HTTP adapter. Constructed by each adapter
/// crate's `into_lifecycle` implementation; owns the concrete state
/// needed to serve and shut down. The orchestrator only sees the
/// [`ServerLifecycle`] surface.
pub struct HttpLifecycleHandle {
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl HttpLifecycleHandle {
    /// Build a handle from the local address, the long-running serve
    /// future, and a callback that triggers graceful shutdown.
    pub fn new<F, Fut>(
        local_addr: SocketAddr,
        serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        shutdown: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            local_addr,
            serve: Some(serve),
            shutdown: Some(Box::new(move || Box::pin(shutdown()))),
        }
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
        if let Some(cb) = self.shutdown.take() {
            cb().await
        } else {
            Ok(())
        }
    }
}

// ─── WebSocket (separate-port) ──────────────────────────────────────────────
//
// One handle per unique separate-port listener. A single adapter produces
// N handles inside `WebSocketAdapter::into_lifecycle_handles`; each handle
// gets a clone of the adapter's shutdown signal in its callback, so calling
// `shutdown` on any handle flips the watch and every port wakes up to
// drain. Idempotent by construction — `watch::Sender::send(true)` after
// the value is already `true` is a no-op.

pub struct WsLifecycleHandle {
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl WsLifecycleHandle {
    pub fn new<F, Fut>(
        local_addr: SocketAddr,
        serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        shutdown: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            local_addr,
            serve: Some(serve),
            shutdown: Some(Box::new(move || Box::pin(shutdown()))),
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
        if let Some(cb) = self.shutdown.take() {
            cb().await
        } else {
            Ok(())
        }
    }
}

// ─── RPC ─────────────────────────────────────────────────────────────────────

pub struct RpcLifecycleHandle {
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl RpcLifecycleHandle {
    pub fn new<F, Fut>(
        local_addr: Option<SocketAddr>,
        serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        shutdown: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            local_addr,
            serve: Some(serve),
            shutdown: Some(Box::new(move || Box::pin(shutdown()))),
        }
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
        if let Some(cb) = self.shutdown.take() {
            cb().await
        } else {
            Ok(())
        }
    }
}

// ─── gRPC ────────────────────────────────────────────────────────────────────

pub(crate) struct GrpcLifecycleHandle {
    adapter: Box<dyn GrpcAdapter>,
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl GrpcLifecycleHandle {
    pub(crate) fn bind(
        mut adapter: Box<dyn GrpcAdapter>,
        services: Vec<(Arc<Box<dyn GrpcServiceTrait>>, Arc<ResolvedGrpcEnhancers>)>,
    ) -> Result<Self> {
        adapter.bind(services)?;
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
