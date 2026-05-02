//! Trait that user-defined gRPC services implement (via `#[grpc_methods]`).
//!
//! Discovery shape mirrors the existing RPC pattern: each service is
//! registered as a provider with `ProviderRole::GrpcService`, the framework
//! collects them at bind time, and the gRPC adapter calls
//! [`register_with`](GrpcServiceTrait::register_with) on each, passing an
//! opaque `&mut dyn Any` registrar.
//!
//! The `dyn Any` indirection keeps tonic types out of toni core. The macro
//! emits a body that downcasts the registrar to a `tonic::service::RoutesBuilder`
//! and calls `add_service(MyServiceServer::new(self.clone()))`. Toni core
//! never needs to mention tonic.

/// gRPC service marker trait — implemented by `#[grpc_methods]` on the
/// trait-impl block. The framework discovers implementors via the DI
/// container at bind time.
pub trait GrpcServiceTrait: Send + Sync + 'static {
    /// Stable token for DI resolution. Defaults to the type name.
    fn token(&self) -> String;

    /// Register this service with the gRPC adapter's routes builder.
    ///
    /// The `registrar` is `&mut tonic::service::RoutesBuilder` boxed as
    /// `dyn Any` so toni core stays tonic-free. The macro-generated impl
    /// downcasts and adds itself.
    fn register_with(&self, registrar: &mut dyn std::any::Any);
}
