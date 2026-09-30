//! Bindings under a slot holding a trait object, handed out as `Arc<S>`.
//!
//! A value's or factory's item is converted to `S` by the cast the caller wrote, and the role it
//! takes is read off `S` alone; a type's own declaration keeps its own roles.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::di::provide::{BoxFuture, Built, IntoFactory, MakeInExecution, Maker};
use crate::di::{DeclaresProvider, Execution, ProviderScope};
use crate::dispatch::transport::{
    Answer, Grpc, GuardEntry, GuardFactory, Http, InterceptorEntry, InterceptorFactory, Rpc, Ws,
};
use crate::enhancer::{ErrorHandler, Guard, Interceptor};
use crate::error::{BuildResult, ResolutionError};
use crate::grpc::GrpcContext;
use crate::http::HttpContext;
use crate::http::middleware::Middleware;
use crate::rpc::RpcContext;
use crate::spi::{Provider, ProviderFactory, ProviderRole, Registration, Shape};
use crate::ws::WsContext;

/// What a value declaration becomes under a slot holding `S`: one item, given where it is declared.
pub struct Binding<S: ?Sized> {
    token: String,
    item: Arc<S>,
}

impl<S: ?Sized + Send + Sync + 'static> Binding<S> {
    pub(crate) fn new(token: String, item: Arc<S>) -> Self {
        Self { token, item }
    }
}

struct BindingProvider<S: ?Sized> {
    token: String,
    item: Arc<S>,
}

#[async_trait]
impl<S: ?Sized + Send + Sync + 'static> Provider for BindingProvider<S> {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.item.clone()))
    }

    fn shape(&self) -> Shape {
        Shape::Shared
    }
}

#[async_trait]
impl<S: ?Sized + Send + Sync + 'static> ProviderFactory for Binding<S> {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let provider: Arc<dyn Provider> = Arc::new(BindingProvider {
            token: self.token.clone(),
            item: self.item.clone(),
        });
        Ok(Registration::new(provider, slot_roles(&self.item)))
    }
}

/// What a factory declaration becomes under a slot holding `S`: a singleton, built once at
/// startup. [`per_execution`](Self::per_execution) and [`transient`](Self::transient) build it
/// again inside each execution, or at each resolution.
pub struct FactoryBinding<S: ?Sized, A, F: IntoFactory<A>> {
    token: String,
    factory: Arc<F>,
    cast: fn(Arc<F::Output>) -> Arc<S>,
    _args: PhantomData<fn(A)>,
}

impl<S, A, F> FactoryBinding<S, A, F>
where
    S: ?Sized + Send + Sync + 'static,
    F: IntoFactory<A>,
    A: 'static,
{
    pub(crate) fn new(token: String, factory: Arc<F>, cast: fn(Arc<F::Output>) -> Arc<S>) -> Self {
        Self {
            token,
            factory,
            cast,
            _args: PhantomData,
        }
    }

    /// Built once inside each execution and shared by everything in it that asks, in a guard or
    /// interceptor slot's per-execution arm; no other trait-object slot has one.
    pub fn per_execution(self) -> ScopedBinding<S, A, F>
    where
        S: PerExecution,
    {
        self.scoped(ProviderScope::Execution)
    }

    /// Built at every resolution. See [`per_execution`](Self::per_execution).
    pub fn transient(self) -> ScopedBinding<S, A, F>
    where
        S: PerExecution,
    {
        self.scoped(ProviderScope::Transient)
    }

    fn scoped(self, scope: ProviderScope) -> ScopedBinding<S, A, F> {
        ScopedBinding {
            token: self.token,
            factory: self.factory,
            cast: self.cast,
            scope,
            _args: PhantomData,
        }
    }
}

#[async_trait]
impl<S, A, F> ProviderFactory for FactoryBinding<S, A, F>
where
    S: ?Sized + Send + Sync + 'static,
    F: IntoFactory<A>,
    A: 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        F::dependency_tokens()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let built: Built = deps.into_iter().map(|(k, v)| (k, v.instance)).collect();
        let item = (self.cast)(Arc::new(self.factory.call(&built, Execution::None).await?));
        let roles = slot_roles(&item);
        let provider: Arc<dyn Provider> = Arc::new(BindingProvider {
            token: self.token.clone(),
            item,
        });
        Ok(Registration::new(provider, roles))
    }
}

/// What [`FactoryBinding::per_execution`] and [`FactoryBinding::transient`] declare.
pub struct ScopedBinding<S: ?Sized, A, F: IntoFactory<A>> {
    token: String,
    factory: Arc<F>,
    cast: fn(Arc<F::Output>) -> Arc<S>,
    scope: ProviderScope,
    _args: PhantomData<fn(A)>,
}

/// The factory's instance for an execution, converted to what the slot holds. The cast keeps the
/// allocation, so everything in one execution holds one instance.
struct Converted<V, S: ?Sized> {
    make: Arc<dyn MakeInExecution<V>>,
    cast: fn(Arc<V>) -> Arc<S>,
}

impl<V: Send + Sync + 'static, S: ?Sized + Send + Sync + 'static> MakeInExecution<S>
    for Converted<V, S>
{
    fn make(&self, execution: Execution) -> BoxFuture<'_, Result<Arc<S>, ResolutionError>> {
        Box::pin(async move { Ok((self.cast)(self.make.make(execution).await?)) })
    }
}

struct ScopedBindingProvider<S: ?Sized> {
    token: String,
    scope: ProviderScope,
    make: Arc<dyn MakeInExecution<S>>,
}

#[async_trait]
impl<S: ?Sized + Send + Sync + 'static> Provider for ScopedBindingProvider<S> {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn scope(&self) -> ProviderScope {
        self.scope
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.make.make(ctx).await?))
    }

    fn shape(&self) -> Shape {
        Shape::Shared
    }
}

#[async_trait]
impl<S, A, F> ProviderFactory for ScopedBinding<S, A, F>
where
    S: ?Sized + PerExecution + Send + Sync,
    F: IntoFactory<A>,
    A: 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        F::dependency_tokens()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let built: Built = deps.into_iter().map(|(k, v)| (k, v.instance)).collect();
        let make: Arc<dyn MakeInExecution<S>> = Arc::new(Converted {
            make: Arc::new(Maker::new(
                self.factory.clone(),
                built,
                self.scope,
                self.token.clone(),
            )) as Arc<dyn MakeInExecution<F::Output>>,
            cast: self.cast,
        });
        let roles = S::roles(&make);
        let provider: Arc<dyn Provider> = Arc::new(ScopedBindingProvider {
            token: self.token.clone(),
            scope: self.scope,
            make,
        });
        Ok(Registration::new(provider, roles))
    }
}

/// What a type's own declaration becomes under a slot holding `S`. It is built as the type
/// declares, in the type's own scope: a singleton's instance is converted once, and any other
/// scope converts at each resolution. The type's roles and lifecycle hooks come with it.
pub struct Recast<T, S: ?Sized> {
    token: String,
    cast: fn(Arc<T>) -> Arc<S>,
}

impl<T, S: ?Sized> Recast<T, S> {
    pub(crate) fn new(token: String, cast: fn(Arc<T>) -> Arc<S>) -> Self {
        Self { token, cast }
    }
}

struct RecastProvider<T, S: ?Sized> {
    token: String,
    inner: Arc<dyn Provider>,
    cast: fn(Arc<T>) -> Arc<S>,
    /// A singleton's one instance, converted once so every injection site holds it.
    shared: Option<Arc<S>>,
}

impl<T: 'static, S: ?Sized + 'static> RecastProvider<T, S> {
    /// The inner provider's instance converted to `S`. A shared instance keeps its allocation, so
    /// the slot hands out the instance the inner provider built and runs its hooks on.
    async fn convert(
        inner: &Arc<dyn Provider>,
        cast: fn(Arc<T>) -> Arc<S>,
        ctx: Execution,
    ) -> Result<Arc<S>, ResolutionError> {
        let token = crate::di::token_of::<T>();
        Ok(cast(crate::__di::take_shared(
            inner.resolve(ctx).await?,
            &token,
        )?))
    }
}

#[async_trait]
impl<T, S> Provider for RecastProvider<T, S>
where
    T: Send + Sync + 'static,
    S: ?Sized + Send + Sync + 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    fn scope(&self) -> ProviderScope {
        self.inner.scope()
    }

    fn shape(&self) -> Shape {
        Shape::Shared
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(match &self.shared {
            Some(item) => Box::new(item.clone()),
            None => Box::new(Self::convert(&self.inner, self.cast, ctx).await?),
        })
    }

    async fn on_module_init(&self) -> crate::di::InitResult {
        self.inner.on_module_init().await
    }

    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        self.inner.on_application_bootstrap().await
    }

    async fn on_module_destroy(&self) {
        self.inner.on_module_destroy().await
    }

    async fn before_application_shutdown(&self, signal: Option<String>) {
        self.inner.before_application_shutdown(signal).await
    }

    async fn on_application_shutdown(&self, signal: Option<String>) {
        self.inner.on_application_shutdown(signal).await
    }
}

#[async_trait]
impl<T, S> ProviderFactory for Recast<T, S>
where
    T: DeclaresProvider + Send + Sync + 'static,
    S: ?Sized + Send + Sync + 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        T::provider_factory().dependency_tokens()
    }

    fn value_dependencies(&self) -> Vec<(String, &'static str)> {
        T::provider_factory().value_dependencies()
    }

    fn identity_hint(&self) -> Option<String> {
        T::provider_factory().identity_hint()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let Registration { instance, roles } = T::provider_factory().build(deps).await?;
        let shared = match instance.scope() {
            ProviderScope::Singleton => {
                Some(RecastProvider::convert(&instance, self.cast, Execution::None).await?)
            }
            _ => None,
        };
        let provider: Arc<dyn Provider> = Arc::new(RecastProvider {
            token: self.token.clone(),
            inner: instance,
            cast: self.cast,
            shared,
        });
        Ok(Registration::new(provider, roles))
    }
}

/// A trait-object slot whose binding may be built again for every execution, and the roles such a
/// binding takes.
///
/// A guard or interceptor slot takes the binding in its per-execution arm. An error handler and
/// middleware have one instance each, and a slot holding any other trait object is filled once. A
/// slot holding a value is scoped through [`Under`](crate::di::Under), where it keeps the value's
/// own shape.
#[diagnostic::on_unimplemented(
    message = "a binding under a slot holding `{Self}` is built once, not per execution or per resolution",
    label = "among trait-object slots, only a guard or interceptor slot is built again",
    note = "declare the factory as a singleton"
)]
pub trait PerExecution: 'static {
    #[doc(hidden)]
    fn roles(make: &Arc<dyn MakeInExecution<Self>>) -> Vec<ProviderRole>;
}

// A per-execution guard or interceptor is built inside a live execution, which leaves `make`
// nothing to refuse but a dependency of another type. The panic unwinds from `create`, which runs
// outside the recovery around `can_activate` and `intercept`, so the chain is not offered it as
// this enhancer's panic.
macro_rules! per_execution_roles {
    ($($transport:ident, $context:ident, $guard:ident, $interceptor:ident;)*) => {$(
        per_execution_roles!(@guard $transport, $context, $guard, dyn Guard<$context>);
        per_execution_roles!(@guard $transport, $context, $guard, dyn Guard<$context> + Send + Sync);
        per_execution_roles!(
            @interceptor $transport, $context, $interceptor,
            dyn Interceptor<$context, Answer<$transport>>
        );
        per_execution_roles!(
            @interceptor $transport, $context, $interceptor,
            dyn Interceptor<$context, Answer<$transport>> + Send + Sync
        );
    )*};
    (@guard $transport:ident, $context:ident, $guard:ident, $held:ty) => {
        impl PerExecution for $held {
            fn roles(make: &Arc<dyn MakeInExecution<$held>>) -> Vec<ProviderRole> {
                struct PerExecutionGuard(Arc<dyn MakeInExecution<$held>>);
                impl GuardFactory<$transport> for PerExecutionGuard {
                    fn create<'a>(
                        &'a self,
                        ctx: &'a $context,
                    ) -> BoxFuture<'a, Arc<dyn Guard<$context> + Send + Sync>> {
                        Box::pin(async move {
                            let guard: Arc<dyn Guard<$context> + Send + Sync> = self
                                .0
                                .make(Execution::from(ctx.clone()))
                                .await
                                .unwrap_or_else(|error| panic!("{error}"));
                            guard
                        })
                    }
                }
                vec![ProviderRole::$guard(GuardEntry::<$transport>::Factory(Arc::new(
                    PerExecutionGuard(make.clone()),
                )))]
            }
        }
    };
    (@interceptor $transport:ident, $context:ident, $interceptor:ident, $held:ty) => {
        impl PerExecution for $held {
            fn roles(make: &Arc<dyn MakeInExecution<$held>>) -> Vec<ProviderRole> {
                struct PerExecutionInterceptor(Arc<dyn MakeInExecution<$held>>);
                impl InterceptorFactory<$transport> for PerExecutionInterceptor {
                    fn create<'a>(
                        &'a self,
                        ctx: &'a $context,
                    ) -> BoxFuture<
                        'a,
                        Arc<dyn Interceptor<$context, Answer<$transport>> + Send + Sync>,
                    > {
                        Box::pin(async move {
                            let interceptor: Arc<
                                dyn Interceptor<$context, Answer<$transport>> + Send + Sync,
                            > = self
                                .0
                                .make(Execution::from(ctx.clone()))
                                .await
                                .unwrap_or_else(|error| panic!("{error}"));
                            interceptor
                        })
                    }
                }
                vec![ProviderRole::$interceptor(InterceptorEntry::<$transport>::Factory(
                    Arc::new(PerExecutionInterceptor(make.clone())),
                ))]
            }
        }
    };
}

per_execution_roles! {
    Http, HttpContext, HttpGuard, HttpInterceptor;
    Rpc, RpcContext, RpcGuard, RpcInterceptor;
    Ws, WsContext, WsGuard, WsInterceptor;
    Grpc, GrpcContext, GrpcGuard, GrpcInterceptor;
}

/// The role an item takes from the trait object its slot holds: a guard, an interceptor or an
/// error handler of one transport, or middleware. Any other slot takes none.
pub(crate) fn slot_roles<S: ?Sized + 'static>(item: &Arc<S>) -> Vec<ProviderRole> {
    let item: &dyn Any = item;
    macro_rules! read {
        ($($held:ty => |$v:ident| $role:expr;)*) => {$(
            if let Some($v) = item.downcast_ref::<Arc<$held>>() {
                let $v = $v.clone();
                return vec![$role];
            }
        )*};
    }
    macro_rules! transports {
        ($($transport:ident, $context:ident, $guard:ident, $interceptor:ident, $handler:ident;)*) => {$(
            read! {
                dyn Guard<$context> => |g| ProviderRole::$guard(GuardEntry::<$transport>::Ready(g));
                dyn Guard<$context> + Send + Sync
                    => |g| ProviderRole::$guard(GuardEntry::<$transport>::Ready(g));
                dyn Interceptor<$context, Answer<$transport>>
                    => |i| ProviderRole::$interceptor(InterceptorEntry::<$transport>::Ready(i));
                dyn Interceptor<$context, Answer<$transport>> + Send + Sync
                    => |i| ProviderRole::$interceptor(InterceptorEntry::<$transport>::Ready(i));
                dyn ErrorHandler<$context, Answer<$transport>> => |h| ProviderRole::$handler(h);
                dyn ErrorHandler<$context, Answer<$transport>> + Send + Sync
                    => |h| ProviderRole::$handler(h);
            }
        )*};
    }
    transports! {
        Http, HttpContext, HttpGuard, HttpInterceptor, HttpErrorHandler;
        Rpc, RpcContext, RpcGuard, RpcInterceptor, RpcErrorHandler;
        Ws, WsContext, WsGuard, WsInterceptor, WsErrorHandler;
        Grpc, GrpcContext, GrpcGuard, GrpcInterceptor, GrpcErrorHandler;
    }
    read! {
        dyn Middleware => |m| ProviderRole::Middleware(m);
        dyn Middleware + Send + Sync => |m| ProviderRole::Middleware(m);
    }
    Vec::new()
}
