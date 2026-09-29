//! Collections: the contributions a key declared with `into` gathers.
//!
//! A collection holds `Vec<Arc<T>>`. Turning a contribution's `Arc<V>` into an `Arc<dyn Trait>`
//! for a trait the caller chooses needs `Unsize`, which is unstable, so no function here
//! converts: each takes the item already converted, or a cast. `provide!` writes that cast where
//! both types are concrete:
//!
//! ```ignore
//! provide!(into dyn Plugin => A {})
//! // expands to
//! Declaration::into_with::<dyn Plugin>(Provide::value(A {}), |item| item)
//! ```

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::di::provide::Built;
use crate::di::{DeclaresProvider, Execution, token_of};
use crate::dispatch::transport::{Answer, Grpc, GuardEntry, Http, InterceptorEntry, Rpc, Ws};
use crate::enhancer::{ErrorHandler, Guard, Interceptor};
use crate::error::{BuildResult, ResolutionError};
use crate::grpc::GrpcContext;
use crate::http::HttpContext;
use crate::http::middleware::Middleware;
use crate::rpc::RpcContext;
use crate::spi::{Provider, ProviderFactory, ProviderRole, Registration};
use crate::ws::WsContext;

/// An item, and the provider it was built through when a type's own declaration built it.
type ItemAndSource<T> = (Arc<T>, Option<Arc<dyn Provider>>);

type BuildItem<T> = Arc<
    dyn Fn(
            Arc<Built>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = BuildResult<ItemAndSource<T>>> + Send>,
        > + Send
        + Sync,
>;

/// Where an item comes from: given, or built once at startup from the dependencies named.
pub(crate) enum Source<T: ?Sized> {
    Item(Arc<T>),
    Built {
        dependencies: Vec<String>,
        build: BuildItem<T>,
    },
}

impl<T: ?Sized> Clone for Source<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Item(item) => Self::Item(item.clone()),
            Self::Built {
                dependencies,
                build,
            } => Self::Built {
                dependencies: dependencies.clone(),
                build: build.clone(),
            },
        }
    }
}

impl<T: ?Sized + Send + Sync + 'static> Source<T> {
    pub(crate) fn dependencies(&self) -> Vec<String> {
        match self {
            Self::Item(_) => Vec::new(),
            Self::Built { dependencies, .. } => dependencies.clone(),
        }
    }

    pub(crate) async fn item(
        &self,
        deps: FxHashMap<String, Registration>,
    ) -> BuildResult<ItemAndSource<T>> {
        match self {
            Self::Item(item) => Ok((item.clone(), None)),
            Self::Built { build, .. } => {
                let built: Built = deps.into_iter().map(|(k, v)| (k, v.instance)).collect();
                build(Arc::new(built)).await
            }
        }
    }
}

/// `V` built through its own declaration: its dependencies, its factory, then its value.
pub(crate) fn declared_source<T, V>(cast: fn(Arc<V>) -> Arc<T>) -> Source<T>
where
    T: ?Sized + Send + Sync + 'static,
    V: DeclaresProvider + Send + 'static,
{
    Source::Built {
        dependencies: V::provider_factory().dependency_tokens(),
        build: Arc::new(move |built: Arc<Built>| {
            Box::pin(async move {
                let deps: FxHashMap<String, Registration> = built
                    .iter()
                    .map(|(k, v)| (k.clone(), Registration::new(v.clone(), Vec::new())))
                    .collect();
                let Registration { instance, .. } = V::provider_factory().build(deps).await?;
                let value = *instance
                    .resolve(Execution::None)
                    .await?
                    .downcast::<V>()
                    .map_err(|_| ResolutionError::TypeMismatch {
                        token: token_of::<V>(),
                    })?;
                Ok((cast(Arc::new(value)), Some(instance)))
            })
        }),
    }
}

static CONTRIBUTIONS: AtomicU64 = AtomicU64::new(0);

/// What [`Provide::into`](crate::di::Provide::into), [`Provide::into_key`](crate::di::Provide::into_key)
/// and [`Declaration::into_with`](crate::di::Declaration::into_with) declare: one contribution to
/// a collection, built at startup.
pub struct Contribution<T: ?Sized> {
    base: String,
    token: String,
    source: Source<T>,
}

impl<T: ?Sized + Send + Sync + 'static> Contribution<T> {
    pub(crate) fn new(base: String, source: Source<T>) -> Self {
        let id = CONTRIBUTIONS.fetch_add(1, Ordering::Relaxed);
        Self {
            token: format!("__ulo_into__{base}__{id}"),
            base,
            source,
        }
    }
}

/// A contribution. `declared` is the provider a type's own declaration built the item through,
/// registered nowhere else, whose lifecycle hooks run through this one.
struct ContributionProvider<T: ?Sized> {
    token: String,
    base: String,
    item: Arc<T>,
    declared: Option<Arc<dyn Provider>>,
}

#[async_trait]
impl<T: ?Sized + Send + Sync + 'static> Provider for ContributionProvider<T> {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn multi_base_token(&self) -> Option<String> {
        Some(self.base.clone())
    }

    fn as_multi_item(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(Arc::new(self.item.clone()))
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.item.clone()))
    }

    async fn on_module_init(&self) -> crate::di::InitResult {
        match &self.declared {
            Some(declared) => declared.on_module_init().await,
            None => Ok(()),
        }
    }

    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        match &self.declared {
            Some(declared) => declared.on_application_bootstrap().await,
            None => Ok(()),
        }
    }

    async fn on_module_destroy(&self) {
        if let Some(declared) = &self.declared {
            declared.on_module_destroy().await
        }
    }

    async fn before_application_shutdown(&self, signal: Option<String>) {
        if let Some(declared) = &self.declared {
            declared.before_application_shutdown(signal).await
        }
    }

    async fn on_application_shutdown(&self, signal: Option<String>) {
        if let Some(declared) = &self.declared {
            declared.on_application_shutdown(signal).await
        }
    }
}

#[async_trait]
impl<T: ?Sized + Send + Sync + 'static> ProviderFactory for Contribution<T> {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        self.source.dependencies()
    }

    fn multi_base_token(&self) -> Option<String> {
        Some(self.base.clone())
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let (item, declared) = self.source.item(deps).await?;
        let roles = collection_roles(&item);
        let provider: Arc<dyn Provider> = Arc::new(ContributionProvider {
            token: self.token.clone(),
            base: self.base.clone(),
            item,
            declared,
        });
        Ok(Registration::new(provider, roles))
    }
}

/// The role a contribution takes from its collection's element type. The unnamed collection of a
/// role type is that transport's global set; see [`global_collection`].
fn collection_roles<T: ?Sized + 'static>(item: &Arc<T>) -> Vec<ProviderRole> {
    let any: &dyn Any = item;
    if let Some(guard) = any.downcast_ref::<Arc<dyn Guard<HttpContext>>>() {
        return vec![ProviderRole::HttpGuard(GuardEntry::<Http>::Ready(
            guard.clone(),
        ))];
    }
    if let Some(guard) = any.downcast_ref::<Arc<dyn Guard<HttpContext> + Send + Sync>>() {
        let guard: Arc<dyn Guard<HttpContext>> = guard.clone();
        return vec![ProviderRole::HttpGuard(GuardEntry::<Http>::Ready(guard))];
    }
    if let Some(i) = any.downcast_ref::<Arc<dyn Interceptor<HttpContext, Answer<Http>>>>() {
        return vec![ProviderRole::HttpInterceptor(
            InterceptorEntry::<Http>::Ready(i.clone()),
        )];
    }
    if let Some(i) =
        any.downcast_ref::<Arc<dyn Interceptor<HttpContext, Answer<Http>> + Send + Sync>>()
    {
        let i: Arc<dyn Interceptor<HttpContext, Answer<Http>>> = i.clone();
        return vec![ProviderRole::HttpInterceptor(
            InterceptorEntry::<Http>::Ready(i),
        )];
    }
    Vec::new()
}

/// Which global set the collection under `base` is, when it is the unnamed collection of a role
/// type the container reads as one.
pub(crate) enum GlobalCollection {
    HttpGuards,
    HttpInterceptors,
}

/// The global set `base` names, or the refusal for a role collection no global set reads.
pub(crate) fn global_collection(base: &str) -> Result<Option<GlobalCollection>, String> {
    if base == token_of::<dyn Guard<HttpContext>>()
        || base == token_of::<dyn Guard<HttpContext> + Send + Sync>()
    {
        return Ok(Some(GlobalCollection::HttpGuards));
    }
    if base == token_of::<dyn Interceptor<HttpContext, Answer<Http>>>()
        || base == token_of::<dyn Interceptor<HttpContext, Answer<Http>> + Send + Sync>()
    {
        return Ok(Some(GlobalCollection::HttpInterceptors));
    }
    let unread: [(String, String, &str); 11] = [
        both::<dyn Guard<RpcContext>, dyn Guard<RpcContext> + Send + Sync>(
            "UloFactory::use_global_rpc_guards",
        ),
        both::<dyn Guard<WsContext>, dyn Guard<WsContext> + Send + Sync>(
            "UloFactory::use_global_ws_guards",
        ),
        both::<dyn Guard<GrpcContext>, dyn Guard<GrpcContext> + Send + Sync>(
            "UloFactory::use_global_grpc_guards",
        ),
        both::<
            dyn Interceptor<RpcContext, Answer<Rpc>>,
            dyn Interceptor<RpcContext, Answer<Rpc>> + Send + Sync,
        >("UloFactory::use_global_rpc_interceptors"),
        both::<
            dyn Interceptor<WsContext, Answer<Ws>>,
            dyn Interceptor<WsContext, Answer<Ws>> + Send + Sync,
        >("UloFactory::use_global_ws_interceptors"),
        both::<
            dyn Interceptor<GrpcContext, Answer<Grpc>>,
            dyn Interceptor<GrpcContext, Answer<Grpc>> + Send + Sync,
        >("UloFactory::use_global_grpc_interceptors"),
        both::<
            dyn ErrorHandler<HttpContext, Answer<Http>>,
            dyn ErrorHandler<HttpContext, Answer<Http>> + Send + Sync,
        >("UloFactory::use_global_http_error_handler"),
        both::<
            dyn ErrorHandler<RpcContext, Answer<Rpc>>,
            dyn ErrorHandler<RpcContext, Answer<Rpc>> + Send + Sync,
        >("UloFactory::use_global_rpc_error_handler"),
        both::<
            dyn ErrorHandler<WsContext, Answer<Ws>>,
            dyn ErrorHandler<WsContext, Answer<Ws>> + Send + Sync,
        >("UloFactory::use_global_ws_error_handler"),
        both::<
            dyn ErrorHandler<GrpcContext, Answer<Grpc>>,
            dyn ErrorHandler<GrpcContext, Answer<Grpc>> + Send + Sync,
        >("UloFactory::use_global_grpc_error_handler"),
        both::<dyn Middleware, dyn Middleware + Send + Sync>("UloFactory::use_global_middleware"),
    ];
    for (bare, bounded, surface) in unread {
        if base == bare || base == bounded {
            return Err(format!(
                "`{base}` is a role collection the container does not read as a global set; register \
                 a global of this role with `{surface}`"
            ));
        }
    }
    Ok(None)
}

/// A role collection's two spellings, with and without `+ Send + Sync`, and where its global is
/// registered instead.
fn both<Bare: ?Sized + 'static, Bounded: ?Sized + 'static>(
    surface: &'static str,
) -> (String, String, &'static str) {
    (token_of::<Bare>(), token_of::<Bounded>(), surface)
}
