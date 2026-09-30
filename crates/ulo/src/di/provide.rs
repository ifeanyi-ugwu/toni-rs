//! The value API a provider declaration is: what `provide!` expands to, and what a function
//! building declarations calls directly.
//!
//! ```ignore
//! key!(pub Primary: Pool);
//! key!(pub Auth: dyn Guard<HttpContext>);
//!
//! fn for_root(config: DbConfig) -> DynamicModule {
//!     DynamicModule::builder("db")
//!         .provider(Provide::value(config))                                   // under DbConfig
//!         .provider(Provide::factory(async |cfg: Arc<DbConfig>| Pool::open(&cfg.url).await))
//!         .provider(Provide::alias_key::<Primary, Pool>())
//!         .provider(AuditGuard::provide().under_key_with::<Auth>(|guard| guard))
//!         .build()
//! }
//! ```
//!
//! A value, a factory and a type's own singleton or execution-scoped declaration hand out one
//! shared `Arc`; a transient hands out a fresh value.
//!
//! A declaration binds under the type it builds unless a key names another slot. A type is
//! addressed by a type parameter and a marker through a `_key` form. What may be bound under a
//! slot is what its [`Key::Value`] says: a value of that type, or under a trait object anything
//! converting to it. The conversion to a trait object needs both types concrete, so the forms
//! binding one take a cast, which `provide!` writes. Collections are in `collection`.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::di::binding::{Binding, FactoryBinding, Recast};
use crate::di::collection::{Contribution, Source, declared_source};
use crate::di::{DeclaresProvider, Execution, Key, ProviderScope, token_of};
use crate::error::{BuildResult, ResolutionError};
use crate::spi::{Provider, ProviderFactory, Registration, Shape};

pub(crate) type Built = FxHashMap<String, Arc<dyn Provider>>;
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Constructors of a provider declaration: a value, a factory, an alias, or a collection item
/// already converted. A type's own declaration is [`DeclaresProvider::provide`].
pub struct Provide;

impl Provide {
    /// A value under its own type, built once where it is written and handed out as one shared
    /// `Arc<V>`.
    pub fn value<V: Send + Sync + 'static>(value: V) -> ValueDeclaration<V> {
        ValueDeclaration {
            value: Arc::new(value),
        }
    }

    /// A value under the marker `K`, whose slot holds the value's type.
    pub fn value_key<K>(value: K::Value) -> Under<ValueDeclaration<K::Value>>
    where
        K: Key,
        K::Value: Sized + Send + Sync,
    {
        Provide::value(value).under_key::<K>()
    }

    /// A value built by an async function, once, under the type it builds, and handed out as one
    /// shared `Arc`.
    ///
    /// Every parameter is written `Arc<Dep>` and resolved by `Dep`. A sync closure is refused: a
    /// factory is always async, and one written sync gains `async` in front of its bars.
    pub fn factory<A, F: IntoFactory<A>>(factory: F) -> FactoryDeclaration<A, F> {
        FactoryDeclaration {
            factory: Arc::new(factory),
            _args: PhantomData,
        }
    }

    /// A factory under the marker `K`, whose slot holds what it builds.
    pub fn factory_key<K, A, F>(factory: F) -> Under<FactoryDeclaration<A, F>>
    where
        K: Key,
        F: IntoFactory<A, Output = K::Value>,
        A: 'static,
    {
        Provide::factory(factory).under_key::<K>()
    }

    /// `T`'s slot answering with the binding under `E`: its scope, its roles and its value are
    /// that binding's. What `E` holds is checked when the alias resolves.
    pub fn alias<T: ?Sized + 'static, E: ?Sized + 'static>() -> AliasDeclaration {
        AliasDeclaration {
            token: token_of::<T>(),
            existing: token_of::<E>(),
        }
    }

    /// The marker `K`'s slot answering with the binding under `E`. See [`alias`](Self::alias).
    pub fn alias_key<K: Key, E: ?Sized + 'static>() -> AliasDeclaration {
        AliasDeclaration {
            token: token_of::<K>(),
            existing: token_of::<E>(),
        }
    }

    /// A contribution to the collection of `T`, the item already converted.
    ///
    /// `#[inject] plugins: Vec<Arc<dyn Plugin>>` reads the collection `dyn Plugin`.
    pub fn into<T: ?Sized + Send + Sync + 'static>(item: Arc<T>) -> Contribution<T> {
        Contribution::new(token_of::<T>(), Source::Item(item))
    }

    /// A contribution to the collection under the marker `K`, whose `Value` is the element type.
    pub fn into_key<K>(item: Arc<K::Value>) -> Contribution<K::Value>
    where
        K: Key,
        K::Value: Send + Sync,
    {
        Contribution::new(token_of::<K>(), Source::Item(item))
    }
}

/// What a declaration takes to be bound under another slot or to contribute to a collection.
///
/// Implemented by what [`Provide::value`], [`Provide::factory`] and
/// [`DeclaresProvider::provide`] return. A slot holding the declared type takes it as it is,
/// through [`under_key`](Self::under_key); a slot holding a trait object takes it through a cast
/// from `Arc<Output>`, which only code naming both types can write: `|item| item`.
pub trait Declaration: Sized + 'static {
    /// What the declaration builds.
    type Output: Send + Sync + 'static;

    /// This declaration bound under a slot holding `S`.
    type Held<S: ?Sized + Send + Sync + 'static>: ProviderFactory;

    #[doc(hidden)]
    fn __hold<S: ?Sized + Send + Sync + 'static>(
        self,
        token: String,
        cast: fn(Arc<Self::Output>) -> Arc<S>,
    ) -> Self::Held<S>;

    #[doc(hidden)]
    fn __contribute<S: ?Sized + Send + Sync + 'static>(
        self,
        base: String,
        cast: fn(Arc<Self::Output>) -> Arc<S>,
    ) -> Contribution<S>;

    /// Bound under the marker `K` instead of its own type. Its roles, its scope and its lifecycle
    /// hooks come with it, and it is handed out as it would be under its own type.
    fn under_key<K: Key<Value = Self::Output>>(self) -> Under<Self> {
        Under::new(self, token_of::<K>())
    }

    /// Bound under the marker `K`, whose slot holds a trait object `cast` converts to, and handed
    /// out as `Arc<K::Value>`. A type's own declaration keeps its roles; a value or factory takes
    /// the role `K::Value` names, such as a guard's under `dyn Guard<HttpContext>`.
    fn under_key_with<K>(self, cast: fn(Arc<Self::Output>) -> Arc<K::Value>) -> Self::Held<K::Value>
    where
        K: Key,
        K::Value: Send + Sync,
    {
        self.__hold(token_of::<K>(), cast)
    }

    /// Bound under the trait object `T`'s own slot, and handed out as `Arc<T>`:
    /// `#[inject] logger: Arc<dyn Logger>` reads what `ConsoleLogger::provide().under_with::<dyn
    /// Logger>(|l| l)` binds.
    fn under_with<T: ?Sized + Send + Sync + 'static>(
        self,
        cast: fn(Arc<Self::Output>) -> Arc<T>,
    ) -> Self::Held<T> {
        self.__hold(token_of::<T>(), cast)
    }

    /// A contribution to the collection of `T`, built once at startup. A type's own declaration
    /// contributes a second instance beside any `providers: [T]` registration, and runs its
    /// lifecycle hooks. Its own roles do not come with it; it takes the collection's role, if any.
    fn into_with<T: ?Sized + Send + Sync + 'static>(
        self,
        cast: fn(Arc<Self::Output>) -> Arc<T>,
    ) -> Contribution<T> {
        self.__contribute(token_of::<T>(), cast)
    }

    /// A contribution to the collection under the marker `K`. See [`into_with`](Self::into_with).
    fn into_key_with<K>(
        self,
        cast: fn(Arc<Self::Output>) -> Arc<K::Value>,
    ) -> Contribution<K::Value>
    where
        K: Key,
        K::Value: Send + Sync,
    {
        self.__contribute(token_of::<K>(), cast)
    }
}

/// What [`Provide::value`] declares.
pub struct ValueDeclaration<V> {
    value: Arc<V>,
}

pub(crate) struct ValueProvider<V> {
    pub(crate) token: String,
    pub(crate) value: Arc<V>,
}

#[async_trait]
impl<V: Send + Sync + 'static> Provider for ValueProvider<V> {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.value.clone()))
    }

    fn shape(&self) -> Shape {
        Shape::Shared
    }
}

#[async_trait]
impl<V: Send + Sync + 'static> ProviderFactory for ValueDeclaration<V> {
    fn token(&self) -> String {
        token_of::<V>()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let provider: Arc<dyn Provider> = Arc::new(ValueProvider {
            token: token_of::<V>(),
            value: self.value.clone(),
        });
        Ok(Registration::new(provider, Vec::new()))
    }
}

impl<V: Send + Sync + 'static> Declaration for ValueDeclaration<V> {
    type Output = V;
    type Held<S: ?Sized + Send + Sync + 'static> = Binding<S>;

    fn __hold<S: ?Sized + Send + Sync + 'static>(
        self,
        token: String,
        cast: fn(Arc<V>) -> Arc<S>,
    ) -> Binding<S> {
        Binding::new(token, cast(self.value))
    }

    fn __contribute<S: ?Sized + Send + Sync + 'static>(
        self,
        base: String,
        cast: fn(Arc<V>) -> Arc<S>,
    ) -> Contribution<S> {
        Contribution::new(base, Source::Item(cast(self.value)))
    }
}

/// An async function whose parameters are the dependencies it is built from.
///
/// Implemented for every `Fn(Arc<A1>, .., Arc<An>) -> Fut` up to eight parameters, where `Fut` is a
/// `Send` future. Each parameter is resolved by the type it shares, `token_of::<A>()`, and handed
/// over as an `Arc<A>` field takes it: a shared instance as it is, a value handed out by value,
/// wrapped.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an async factory",
    label = "a factory is an async function of its dependencies, each an `Arc`",
    note = "write `async |dep: Arc<Dep>| ..` or `|dep: Arc<Dep>| async move {{ .. }}`; a sync closure gains `async`"
)]
pub trait IntoFactory<A>: Send + Sync + 'static {
    /// What the factory builds.
    type Output: Send + Sync + 'static;

    /// The token of every parameter, in order.
    fn dependency_tokens() -> Vec<String>;

    /// Resolves the parameters from `built` inside `execution`, and calls the factory. The first
    /// parameter that cannot be resolved there answers with its failure, and the factory is not
    /// called.
    fn call<'a>(
        &'a self,
        built: &'a Built,
        execution: Execution,
    ) -> BoxFuture<'a, Result<Self::Output, ResolutionError>>;
}

macro_rules! into_factory {
    ($($arg:ident),*) => {
        impl<Func, Fut, R, $($arg),*> IntoFactory<($($arg,)*)> for Func
        where
            Func: Fn($(Arc<$arg>),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = R> + Send,
            R: Send + Sync + 'static,
            $($arg: Send + Sync + 'static,)*
        {
            type Output = R;

            fn dependency_tokens() -> Vec<String> {
                vec![$(token_of::<$arg>()),*]
            }

            #[allow(non_snake_case, unused_variables)]
            fn call<'a>(
                &'a self,
                built: &'a Built,
                execution: Execution,
            ) -> BoxFuture<'a, Result<R, ResolutionError>> {
                Box::pin(async move {
                    $(let $arg = resolve_dependency::<$arg>(built, execution.clone()).await?;)*
                    Ok((self)($($arg),*).await)
                })
            }
        }
    };
}

into_factory!();
into_factory!(A1);
into_factory!(A1, A2);
into_factory!(A1, A2, A3);
into_factory!(A1, A2, A3, A4);
into_factory!(A1, A2, A3, A4, A5);
into_factory!(A1, A2, A3, A4, A5, A6);
into_factory!(A1, A2, A3, A4, A5, A6, A7);
into_factory!(A1, A2, A3, A4, A5, A6, A7, A8);

/// One parameter of a factory, resolved by the type it shares.
///
/// The loader builds a declaration only after every token in `dependency_tokens` is built, so an
/// entry is always present. A value of another type means some declaration registered it under a
/// key spelling this type's name.
async fn resolve_dependency<A: 'static>(
    built: &Built,
    execution: Execution,
) -> Result<Arc<A>, ResolutionError> {
    let token = token_of::<A>();
    let provider = built
        .get(&token)
        .unwrap_or_else(|| panic!("the loader built `{token}` before its dependents"));
    crate::__di::take_shared(provider.resolve(execution).await?, &token)
}

/// What [`Provide::factory`] declares: a singleton, built once at startup.
///
/// [`per_execution`](Self::per_execution) and [`transient`](Self::transient) build it again inside
/// each execution, or at each resolution.
pub struct FactoryDeclaration<A, F> {
    pub(crate) factory: Arc<F>,
    pub(crate) _args: PhantomData<fn(A)>,
}

impl<A, F> FactoryDeclaration<A, F>
where
    F: IntoFactory<A>,
    A: 'static,
{
    /// Built once inside each execution and shared by everything in it that asks, as one `Arc`.
    /// Outside an execution it answers [`ResolutionError::ExecutionRequired`], and a singleton
    /// depending on it fails its build.
    pub fn per_execution(self) -> ScopedFactoryDeclaration<A, F> {
        ScopedFactoryDeclaration {
            factory: self.factory,
            scope: ProviderScope::Execution,
            take: |shared| Box::new(shared),
            _args: PhantomData,
        }
    }

    /// Built at every resolution, and handed out by value.
    pub fn transient(self) -> ScopedFactoryDeclaration<A, F> {
        ScopedFactoryDeclaration {
            factory: self.factory,
            scope: ProviderScope::Transient,
            take: |fresh| {
                Box::new(
                    Arc::try_unwrap(fresh).unwrap_or_else(|_| {
                        panic!("a transient value is built for one resolution")
                    }),
                )
            },
            _args: PhantomData,
        }
    }
}

#[async_trait]
impl<A, F> ProviderFactory for FactoryDeclaration<A, F>
where
    F: IntoFactory<A>,
    A: 'static,
{
    fn token(&self) -> String {
        token_of::<F::Output>()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        F::dependency_tokens()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let built: Built = deps.into_iter().map(|(k, v)| (k, v.instance)).collect();
        let value = Arc::new(self.factory.call(&built, Execution::None).await?);
        let provider: Arc<dyn Provider> = Arc::new(ValueProvider {
            token: token_of::<F::Output>(),
            value,
        });
        Ok(Registration::new(provider, Vec::new()))
    }
}

impl<A: 'static, F: IntoFactory<A>> Declaration for FactoryDeclaration<A, F> {
    type Output = F::Output;
    type Held<S: ?Sized + Send + Sync + 'static> = FactoryBinding<S, A, F>;

    fn __hold<S: ?Sized + Send + Sync + 'static>(
        self,
        token: String,
        cast: fn(Arc<F::Output>) -> Arc<S>,
    ) -> FactoryBinding<S, A, F> {
        FactoryBinding::new(token, self.factory, cast)
    }

    fn __contribute<S: ?Sized + Send + Sync + 'static>(
        self,
        base: String,
        cast: fn(Arc<F::Output>) -> Arc<S>,
    ) -> Contribution<S> {
        let factory = self.factory;
        Contribution::new(
            base,
            Source::Built {
                dependencies: F::dependency_tokens(),
                build: Arc::new(move |built: Arc<Built>| {
                    let factory = factory.clone();
                    Box::pin(async move {
                        let value = factory.call(&built, Execution::None).await?;
                        Ok((cast(Arc::new(value)), None))
                    })
                }),
            },
        )
    }
}

/// What [`FactoryDeclaration::per_execution`] and [`FactoryDeclaration::transient`] declare.
pub struct ScopedFactoryDeclaration<A, F: IntoFactory<A>> {
    factory: Arc<F>,
    scope: ProviderScope,
    /// How the provider answers with the instance: the shared `Arc`, or the fresh value itself.
    take: fn(Arc<F::Output>) -> Box<dyn Any + Send>,
    _args: PhantomData<fn(A)>,
}

/// Builds a `V` inside an execution, or hands back the one already built in it.
pub trait MakeInExecution<V: ?Sized>: Send + Sync {
    fn make(&self, execution: Execution) -> BoxFuture<'_, Result<Arc<V>, ResolutionError>>;
}

/// The factory and what it was built after, which is what a per-execution build needs again.
///
/// `cache_key` names this declaration alone: a token repeats across modules and is hidden by
/// `under_key`, and an alias or a rebinding wraps this same `Maker`, so they share its instance.
/// `token` is what a refusal outside an execution names.
pub(crate) struct Maker<A, F> {
    factory: Arc<F>,
    built: Arc<Built>,
    cache_key: String,
    scope: ProviderScope,
    token: String,
    _args: PhantomData<fn(A)>,
}

static SCOPED_DECLARATIONS: AtomicU64 = AtomicU64::new(0);

impl<A: 'static, F: IntoFactory<A>> Maker<A, F> {
    pub(crate) fn new(factory: Arc<F>, built: Built, scope: ProviderScope, token: String) -> Self {
        Self {
            factory,
            built: Arc::new(built),
            cache_key: format!(
                "__ulo_execution__{}",
                SCOPED_DECLARATIONS.fetch_add(1, Ordering::Relaxed)
            ),
            scope,
            token,
            _args: PhantomData,
        }
    }
}

impl<A: 'static, F: IntoFactory<A>> MakeInExecution<F::Output> for Maker<A, F> {
    fn make(&self, execution: Execution) -> BoxFuture<'_, Result<Arc<F::Output>, ResolutionError>> {
        Box::pin(async move {
            // Per execution, the instance lives in the execution's cache under this declaration's
            // own key, since two declarations may build one type.
            let shared = match self.scope {
                ProviderScope::Execution => match execution.cache() {
                    Some(cache) => Some(cache),
                    None => {
                        return Err(ResolutionError::ExecutionRequired {
                            token: self.token.clone(),
                        });
                    }
                },
                _ => None,
            };
            if let Some(cached) = shared.and_then(|cache| cache.get_keyed(&self.cache_key)) {
                if let Ok(value) = cached.downcast::<F::Output>() {
                    return Ok(value);
                }
            }
            let value = Arc::new(self.factory.call(&self.built, execution.clone()).await?);
            Ok(match shared {
                Some(cache) => cache
                    .insert_keyed(&self.cache_key, value.clone())
                    .downcast::<F::Output>()
                    .unwrap_or(value),
                None => value,
            })
        })
    }
}

struct ScopedProvider<V> {
    token: String,
    scope: ProviderScope,
    make: Arc<dyn MakeInExecution<V>>,
    take: fn(Arc<V>) -> Box<dyn Any + Send>,
}

#[async_trait]
impl<V: Send + Sync + 'static> Provider for ScopedProvider<V> {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn scope(&self) -> ProviderScope {
        self.scope
    }

    fn shape(&self) -> Shape {
        match self.scope {
            ProviderScope::Transient => Shape::Value,
            _ => Shape::Shared,
        }
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok((self.take)(self.make.make(ctx).await?))
    }
}

#[async_trait]
impl<A, F> ProviderFactory for ScopedFactoryDeclaration<A, F>
where
    F: IntoFactory<A>,
    A: 'static,
{
    fn token(&self) -> String {
        token_of::<F::Output>()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        F::dependency_tokens()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let built: Built = deps.into_iter().map(|(k, v)| (k, v.instance)).collect();
        let make: Arc<dyn MakeInExecution<F::Output>> = Arc::new(Maker::new(
            self.factory.clone(),
            built,
            self.scope,
            token_of::<F::Output>(),
        ));
        let provider: Arc<dyn Provider> = Arc::new(ScopedProvider {
            token: token_of::<F::Output>(),
            scope: self.scope,
            make,
            take: self.take,
        });
        Ok(Registration::new(provider, Vec::new()))
    }
}

/// What [`DeclaresProvider::provide`] returns: the type's own declaration, what `providers: [T]`
/// registers.
pub struct Declared<T> {
    _type: PhantomData<fn() -> T>,
}

impl<T> Declared<T> {
    pub(crate) fn new() -> Self {
        Self { _type: PhantomData }
    }
}

#[async_trait]
impl<T: DeclaresProvider + 'static> ProviderFactory for Declared<T> {
    fn token(&self) -> String {
        T::provider_factory().token()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        T::provider_factory().dependency_tokens()
    }

    fn identity_hint(&self) -> Option<String> {
        T::provider_factory().identity_hint()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        T::provider_factory().build(deps).await
    }
}

impl<T: DeclaresProvider + Send + Sync + 'static> Declaration for Declared<T> {
    type Output = T;
    type Held<S: ?Sized + Send + Sync + 'static> = Recast<T, S>;

    fn __hold<S: ?Sized + Send + Sync + 'static>(
        self,
        token: String,
        cast: fn(Arc<T>) -> Arc<S>,
    ) -> Recast<T, S> {
        Recast::new(token, cast)
    }

    fn __contribute<S: ?Sized + Send + Sync + 'static>(
        self,
        base: String,
        cast: fn(Arc<T>) -> Arc<S>,
    ) -> Contribution<S> {
        Contribution::new(base, declared_source::<S, T>(cast))
    }
}

/// What [`Provide::alias`] and [`Provide::alias_key`] declare.
pub struct AliasDeclaration {
    token: String,
    existing: String,
}

#[async_trait]
impl ProviderFactory for AliasDeclaration {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        vec![self.existing.clone()]
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let Registration { instance, roles } = deps
            .get(&self.existing)
            .cloned()
            .unwrap_or_else(|| panic!("the loader built `{}` before its alias", self.existing));
        let provider: Arc<dyn Provider> = Arc::new(Rekeyed {
            token: self.token.clone(),
            inner: instance,
            stands_in: false,
        });
        Ok(Registration::new(provider, roles))
    }
}

/// A provider answering under another token. Its scope and its answer are the inner provider's.
///
/// `stands_in` is set by [`Under`]: its inner provider is registered nowhere else, and its
/// lifecycle hooks pass through. An alias sits beside the provider it names, which
/// runs its own hooks.
pub(crate) struct Rekeyed {
    pub(crate) token: String,
    pub(crate) inner: Arc<dyn Provider>,
    pub(crate) stands_in: bool,
}

#[async_trait]
impl Provider for Rekeyed {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn scope(&self) -> ProviderScope {
        self.inner.scope()
    }

    fn shape(&self) -> Shape {
        self.inner.shape()
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        self.inner.resolve(ctx).await.map_err(|error| match error {
            // A stand-in's inner provider is registered under no token of its own, so a refusal
            // naming that provider names this slot. Only an execution-scoped inner refuses on its
            // own account; a refusal passing through any other names a dependency.
            ResolutionError::ExecutionRequired { token }
                if self.stands_in
                    && self.inner.scope() == ProviderScope::Execution
                    && token == self.inner.token() =>
            {
                ResolutionError::ExecutionRequired {
                    token: self.token.clone(),
                }
            }
            other => other,
        })
    }

    async fn on_module_init(&self) -> crate::di::InitResult {
        if self.stands_in {
            self.inner.on_module_init().await
        } else {
            Ok(())
        }
    }

    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        if self.stands_in {
            self.inner.on_application_bootstrap().await
        } else {
            Ok(())
        }
    }

    async fn on_module_destroy(&self) {
        if self.stands_in {
            self.inner.on_module_destroy().await
        }
    }

    async fn before_application_shutdown(&self, signal: Option<String>) {
        if self.stands_in {
            self.inner.before_application_shutdown(signal).await
        }
    }

    async fn on_application_shutdown(&self, signal: Option<String>) {
        if self.stands_in {
            self.inner.on_application_shutdown(signal).await
        }
    }
}

/// What [`Declaration::under_key`] declares.
pub struct Under<F> {
    inner: F,
    token: String,
}

impl<F> Under<F> {
    pub(crate) fn new(inner: F, token: String) -> Self {
        Self { inner, token }
    }
}

impl<A, F> Under<FactoryDeclaration<A, F>>
where
    F: IntoFactory<A>,
    A: 'static,
{
    /// See [`FactoryDeclaration::per_execution`].
    pub fn per_execution(self) -> Under<ScopedFactoryDeclaration<A, F>> {
        Under {
            inner: self.inner.per_execution(),
            token: self.token,
        }
    }

    /// See [`FactoryDeclaration::transient`].
    pub fn transient(self) -> Under<ScopedFactoryDeclaration<A, F>> {
        Under {
            inner: self.inner.transient(),
            token: self.token,
        }
    }
}

#[async_trait]
impl<F: ProviderFactory> ProviderFactory for Under<F> {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        self.inner.dependency_tokens()
    }

    fn identity_hint(&self) -> Option<String> {
        self.inner.identity_hint()
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let Registration { instance, roles } = self.inner.build(deps).await?;
        let provider: Arc<dyn Provider> = Arc::new(Rekeyed {
            token: self.token.clone(),
            inner: instance,
            stands_in: true,
        });
        Ok(Registration::new(provider, roles))
    }
}
