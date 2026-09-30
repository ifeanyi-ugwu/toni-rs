use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::di::ProviderScope;
use crate::dispatch::transport::{
    ErrorHandlerArc, Grpc, GuardEntry, Http, InterceptorEntry, Rpc, Ws,
};
use crate::error::{BuildResult, ResolutionError};
use crate::http::middleware::Middleware;

#[async_trait]
pub trait Provider: Send + Sync {
    fn token(&self) -> String;

    /// The value this provider supplies to the execution `ctx` opens.
    ///
    /// A singleton answers with the value built at startup. An execution-scoped provider builds
    /// one per execution and caches it on `ctx`, so everything in the same call that asks for
    /// this token shares it; a transient one builds on every call. The answer is erased —
    /// callers downcast to the concrete type the token stands for.
    ///
    /// An execution-scoped provider must answer [`Execution::None`] with
    /// [`ResolutionError::ExecutionRequired`]: no caller refuses on its scope first. A provider
    /// resolving its own dependencies passes their failure on.
    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError>;
    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }

    /// What [`resolve`](Self::resolve) answers inside the box: one shared `Arc<V>`, or a `V` the
    /// holder owns. A field written as a plain `V` reads only the second. The default is a value,
    /// which is what a provider written by hand to hand out a handle answers, such as a pool or a
    /// client whose clone reaches the same connection.
    fn shape(&self) -> Shape {
        Shape::Value
    }

    fn multi_base_token(&self) -> Option<String> {
        None
    }
    fn as_multi_item(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    // Lifecycle hooks — overridden by the macro when the user annotates a method.
    // Default implementations are no-ops so providers without hooks incur no overhead.
    async fn on_module_init(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_module_destroy(&self) {}
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

/// What a provider hands out: see [`Provider::shape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One instance every holder shares, as an `Arc<V>`: a singleton or execution-scoped
    /// `#[injectable]`, a `provide!` value or factory, a trait object.
    Shared,
    /// A `V` each holder owns: a transient, or a handle whose clone reaches the same thing.
    Value,
}

/// Role trait-objects a provider may contribute to the registry.
///
/// Carried in a [`Registration`] from `ProviderFactory::build`. The container
/// inserts each variant into the matching slot of `RoleRegistry` keyed by the
/// provider token (or, for gateways, by WS path).
#[derive(Clone)]
pub enum ProviderRole {
    HttpGuard(GuardEntry<Http>),
    HttpInterceptor(InterceptorEntry<Http>),
    HttpErrorHandler(ErrorHandlerArc<Http>),

    RpcGuard(GuardEntry<Rpc>),
    RpcInterceptor(InterceptorEntry<Rpc>),
    RpcErrorHandler(ErrorHandlerArc<Rpc>),

    WsGuard(GuardEntry<Ws>),
    WsInterceptor(InterceptorEntry<Ws>),
    WsErrorHandler(ErrorHandlerArc<Ws>),

    GrpcGuard(GuardEntry<Grpc>),
    GrpcInterceptor(InterceptorEntry<Grpc>),
    GrpcErrorHandler(ErrorHandlerArc<Grpc>),

    Middleware(Arc<dyn Middleware>),
    Gateway(Arc<dyn crate::ws::Gateway>),
}

/// What a [`ProviderFactory`] builds: the provider instance and the roles it registers under.
///
/// The loader registers `instance` in its module and each of `roles` in the matching slot of the
/// role registry. A factory's dependencies arrive as their registrations, so a wrapper factory,
/// such as an alias, forwards the roles without a downcast.
#[derive(Clone)]
pub struct Registration {
    pub instance: Arc<dyn Provider>,
    pub roles: Vec<ProviderRole>,
}

impl Registration {
    pub fn new(instance: Arc<dyn Provider>, roles: Vec<ProviderRole>) -> Self {
        Self { instance, roles }
    }
}

#[async_trait]
pub trait ProviderFactory: Send + Sync {
    fn token(&self) -> String;
    fn dependency_tokens(&self) -> Vec<String> {
        vec![]
    }

    /// The dependencies a field or parameter written as a plain type reads, each with the type it
    /// is written as. The loader refuses the build when one of them hands out one shared instance,
    /// [`Shape::Shared`](crate::spi::Shape::Shared), which a consumer built per execution would
    /// otherwise meet at its first resolution.
    fn value_dependencies(&self) -> Vec<(String, &'static str)> {
        vec![]
    }

    fn multi_base_token(&self) -> Option<String> {
        None
    }

    /// A fingerprint of this factory's runtime configuration, folded into the identity of the
    /// `DynamicModule` that carries it.
    ///
    /// Two dynamic modules built from the same maker (e.g. `SeaOrmModule::for_root`) share a base
    /// name but must be distinguished by what they were configured with — a database URL, a pool
    /// size. Return a value derived from that config so identical registrations dedup (the same
    /// module reached through two import paths) while different ones stay distinct. `None` (the
    /// default) leaves identity keyed on the base name alone: two such modules with differing
    /// config collapse into one without an error. Integrations that support multiple instances
    /// should override this. A factory whose configuration cannot be compared, such as a closure,
    /// may return a value unique to each construction; every construction is then a module of its
    /// own, and two exporting one token globally are refused.
    fn identity_hint(&self) -> Option<String> {
        None
    }

    /// The provider, built from `deps`: every token [`dependency_tokens`](Self::dependency_tokens)
    /// names, each already built.
    ///
    /// A singleton builds its value here, outside any execution, and a dependency it cannot
    /// resolve there fails the build; the loader reports it as
    /// [`StartupError::BuildFailed`](crate::error::StartupError::BuildFailed).
    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration>;
}
