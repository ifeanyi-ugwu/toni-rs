use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;

/// Uniform lifecycle protocol for every adapter kind the framework hosts
/// (HTTP, WebSocket, RPC, gRPC, future Kafka/MQTT/etc.).
///
/// The framework's startup/shutdown machinery sees adapters only through
/// this trait. Per-transport dispatch concerns — route binding, pattern
/// registration, service registration — happen during construction of the
/// concrete `*LifecycleHandle` type, not through this trait.
///
/// New adapter kinds plug in by adding a new `*LifecycleHandle: ServerLifecycle`
/// alongside a typed `use_*_adapter()` registration on `ToniApplication`. No
/// orchestration code changes.
#[async_trait]
pub(crate) trait ServerLifecycle: Send + 'static {
    /// Human-readable transport name for logs (e.g. `"http"`, `"rpc"`,
    /// `"grpc"`). Used in `tracing::info!(server = …, …)` calls; must be
    /// stable across framework versions.
    fn name(&self) -> &'static str;

    /// Local listening address, when the transport binds to one. `None` for
    /// subject-based transports (NATS, Kafka) that have no local socket.
    fn local_addr(&self) -> Option<SocketAddr>;

    /// Take ownership of the serve future. Called once by the orchestrator
    /// after `bind()` produces the handle. The framework joins this future
    /// alongside every other adapter's serve future. Subsequent calls
    /// return `None`.
    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    /// Trigger graceful shutdown. The serve future returned by `take_serve`
    /// will resolve shortly after — exactly when depends on the transport's
    /// drain semantics (configured per-adapter, not at the lifecycle layer).
    /// Idempotent; safe to call multiple times.
    async fn shutdown(&mut self) -> Result<()>;
}
