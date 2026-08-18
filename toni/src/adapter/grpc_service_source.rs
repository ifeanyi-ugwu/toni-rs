//! What a gRPC service declares, and how it is registered with tonic.
//!
//! Each service is registered as a provider carrying `ProviderRole::GrpcService`. The framework
//! collects the sources at bind time, resolves their declared enhancer tokens against the role
//! registry, and hands the result back through
//! [`register_with`](GrpcServiceSource::register_with) so the macro-generated body can wrap itself
//! in an enhancer-aware tonic service.
//!
//! Nothing here reaches an instance. Registration happens before any call exists, so a service
//! built per call has nothing to answer with yet; the source knows the declarations without one and
//! produces instances later, inside the call being served.
//!
//! The `dyn Any` registrar keeps tonic types out of toni core. The macro emits a body that
//! downcasts to `tonic::service::RoutesBuilder` and calls
//! `add_service(MyServiceServer::new(wrapper))`, where `wrapper` carries the source and the
//! resolved enhancers. Toni core never names tonic.

use std::sync::Arc;

use crate::traits_helpers::{
    ErrorObserver, GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry,
};

/// Per-service bundle of resolved enhancer instances. Built by the framework
/// at bind time from the token getters on [`GrpcServiceSource`] and handed to
/// [`GrpcServiceSource::register_with`] so the macro-generated wrapper can
/// invoke them per call without touching the DI container at request time.
#[derive(Default, Clone)]
pub struct ResolvedGrpcEnhancers {
    /// Service-level guards; run on every method.
    pub guards: Vec<GrpcGuardEntry>,
    /// Method-level guards keyed by method name (the suffix after `Service/`,
    /// matching what `#[grpc_methods]` emits in `get_handler_methods`).
    pub handler_guards: std::collections::HashMap<String, Vec<GrpcGuardEntry>>,
    /// Service-level interceptors; wrap every method's user delegation.
    pub interceptors: Vec<GrpcInterceptorEntry>,
    /// Method-level interceptors. Stack on top of service-level (controller-
    /// level entries run first, method-level entries run inside).
    pub handler_interceptors: std::collections::HashMap<String, Vec<GrpcInterceptorEntry>>,
    /// Service-level error handlers; fire on user-returned `Err` or caught
    /// handler panic. First handler to claim wins (chain runs in reverse
    /// registration order, matching the RPC/HTTP convention).
    pub error_handlers: Vec<GrpcErrorHandlerArc>,
    /// Method-level error handlers. Composed with service-level into one
    /// reverse-order chain per call.
    pub handler_error_handlers: std::collections::HashMap<String, Vec<GrpcErrorHandlerArc>>,
    /// Universal error observers — fan out on guard rejections, caught
    /// panics, and user-returned errs so logging / telemetry sees gRPC
    /// pipeline events the same way HTTP/RPC/WS do.
    pub error_observers: Vec<Arc<dyn ErrorObserver>>,
}

/// A gRPC service's declarations plus its registration hook — implemented by `#[grpc_methods]` on
/// a companion generated beside the service struct, not on the struct itself. The framework
/// discovers sources through the DI container at bind time.
pub trait GrpcServiceSource: Send + Sync + 'static {
    /// Stable token for DI resolution. Defaults to the type name.
    fn token(&self) -> String;

    /// Register this service with the gRPC adapter's routes builder.
    ///
    /// `registrar` is `&mut tonic::service::RoutesBuilder` boxed as `dyn Any`
    /// so toni core stays tonic-free. The macro-generated impl downcasts and
    /// adds itself, wrapping in an enhancer-aware shim built from
    /// `enhancers`.
    fn register_with(
        &self,
        registrar: &mut dyn std::any::Any,
        enhancers: Arc<ResolvedGrpcEnhancers>,
    );

    // -- Enhancer token getters; default empty so a hand-written source does not
    //    have to touch them. The macro overrides these with the tokens parsed
    //    from `#[use_guards(...)]` etc.

    fn get_guard_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_interceptor_tokens(&self) -> Vec<String> {
        vec![]
    }

    fn get_error_handler_tokens(&self) -> Vec<String> {
        vec![]
    }

    /// Methods carrying any per-method enhancer attribute. Used by the
    /// resolver to know which methods to query the per-handler getters for.
    fn get_handler_methods(&self) -> Vec<String> {
        vec![]
    }

    fn get_handler_guard_tokens(&self, _method: &str) -> Vec<String> {
        vec![]
    }

    fn get_handler_interceptor_tokens(&self, _method: &str) -> Vec<String> {
        vec![]
    }

    fn get_handler_error_handler_tokens(&self, _method: &str) -> Vec<String> {
        vec![]
    }
}
