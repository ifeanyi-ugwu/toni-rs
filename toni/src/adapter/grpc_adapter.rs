use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;

/// Interface for gRPC transport adapters.
///
/// Distinct from [`RpcAdapter`](crate::adapter::RpcAdapter) by design: gRPC is
/// contract-first (services and methods are declared in `.proto` files and
/// known at compile time via `tonic`-generated traits), supports four call
/// shapes (unary + three streaming modes), and dispatches via typed protobuf
/// messages. None of those fit the pattern-string + JSON-data + unary
/// contract that `RpcAdapter` encodes for TCP/UDP/NATS.
///
/// Adapter implementations register tonic services on the wrapped
/// `tonic::transport::Server` *during their own construction* — before
/// `bind()` is called by the framework. The framework only orchestrates the
/// shared lifecycle: bind → serve → close. Per-request dispatch is entirely
/// inside tonic and the user's trait `impl`s.
#[async_trait]
pub trait GrpcAdapter: Send + Sync + 'static {
    /// Acquire the listening socket.
    ///
    /// Called once before `serve`. Implementations should bind synchronously
    /// (so port-in-use surfaces as `Err` from `bind()` rather than panicking
    /// inside the spawned serve loop), capture the local address for
    /// [`local_addr`](GrpcAdapter::local_addr), and prepare the configured
    /// tonic `Server`.
    fn bind(&mut self) -> Result<()>;

    /// Return the future that drives the gRPC serve loop.
    ///
    /// Called once after `bind`. The framework joins this future alongside
    /// every other adapter's serve future. The future resolves when shutdown
    /// has been signalled and the configured drain budget has elapsed.
    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    /// Local listening address. Available after a successful `bind`.
    fn local_addr(&self) -> Option<SocketAddr> {
        None
    }

    /// Trigger graceful shutdown. Idempotent. The serve future returned by
    /// `serve` will resolve once tonic's `serve_with_shutdown` completes —
    /// adapter implementations are responsible for the drain-timeout policy
    /// described in their own docs.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Object-safe internal facade over [`GrpcAdapter`] for storage in
/// `ToniApplication`.
#[async_trait]
pub(crate) trait ErasedGrpcAdapter: Send + Sync + 'static {
    fn bind(&mut self) -> Result<()>;
    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;
    fn local_addr(&self) -> Option<SocketAddr>;
    async fn close(&mut self) -> Result<()>;
}

#[async_trait]
impl<G: GrpcAdapter> ErasedGrpcAdapter for G {
    fn bind(&mut self) -> Result<()> {
        <G as GrpcAdapter>::bind(self)
    }

    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        <G as GrpcAdapter>::serve(self)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        <G as GrpcAdapter>::local_addr(self)
    }

    async fn close(&mut self) -> Result<()> {
        <G as GrpcAdapter>::close(self).await
    }
}
