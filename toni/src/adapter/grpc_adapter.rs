use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::adapter::grpc_service_trait::{GrpcServiceTrait, ResolvedGrpcEnhancers};

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
/// shared lifecycle: `bind` → `into_lifecycle`, then drives the returned
/// handle. Per-request dispatch is entirely inside tonic and the user's
/// trait `impl`s.
#[async_trait]
pub trait GrpcAdapter: Send + Sync + 'static {
    /// Acquire the listening socket and accept any framework-discovered
    /// services for registration.
    ///
    /// Called once before [`into_lifecycle`](Self::into_lifecycle).
    /// Implementations should bind synchronously (so port-in-use surfaces as
    /// `Err` from `bind()` rather than panicking inside the spawned serve
    /// loop), capture the local address for the lifecycle handle, and merge
    /// `services` into the configured tonic `Server`'s routes — each service
    /// contributes itself via [`GrpcServiceTrait::register_with`] using a
    /// tonic `RoutesBuilder` passed as `&mut dyn Any`.
    ///
    /// `services` may be empty when the user wires services directly via
    /// adapter-specific `add_service` calls. Each entry is paired with its
    /// resolved enhancer bundle; the adapter forwards both into
    /// [`GrpcServiceTrait::register_with`].
    fn bind(
        &mut self,
        services: Vec<(Arc<Box<dyn GrpcServiceTrait>>, Arc<ResolvedGrpcEnhancers>)>,
    ) -> Result<()>;

    /// Consume the adapter and return a self-contained lifecycle handle
    /// driving the gRPC serve loop. The handle owns the serve future,
    /// the local address, and a shutdown callback. The framework joins
    /// the serve future alongside every other adapter's serve.
    ///
    /// Implementations should bind synchronously in `bind` so port-in-use
    /// surfaces as `Err` from `app.bind()`, and capture the shutdown
    /// signal in the closure so the framework's `close()` flow flips it
    /// without holding a reference back to the adapter.
    async fn into_lifecycle(
        self: Box<Self>,
    ) -> Result<crate::adapter::lifecycle_handles::GrpcLifecycleHandle>;
}
