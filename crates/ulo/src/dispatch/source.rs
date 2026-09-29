use std::any::Any;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::errors::HookFailed;
use crate::spi::provider::Provider;

/// How a dispatch target's instance is held: built once at startup and shared by every
/// call, or resolved per call from the target's own provider.
///
/// One value of this type sits behind every dispatch target — HTTP controller, RPC
/// controller, gRPC service — and [`resolve`](DispatchSource::resolve) is the one
/// resolution path. The transports differ only in where they call it and which
/// [`Execution`] variant they pass.
pub enum DispatchSource<T> {
    /// Built at startup and shared by every call.
    Singleton(Arc<T>),
    /// The target's own provider, resolved inside the call being served.
    ///
    /// The provider must answer with `Result<Arc<T>, HookFailed>`, fire init/bootstrap at its
    /// own build site — hook resolution needs a concrete-type call site, which the generated
    /// provider body is and this generic code is not — and cache the `Arc` in the execution
    /// only once both hooks return `Ok`. A target asked for twice in one call is then built
    /// once and its hooks fire once; one whose hook failed is left out of the cache.
    PerCall(Arc<Box<dyn Provider>>),
}

impl<T> Clone for DispatchSource<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Singleton(instance) => Self::Singleton(instance.clone()),
            Self::PerCall(provider) => Self::PerCall(provider.clone()),
        }
    }
}

impl<T: Any + Send + Sync> DispatchSource<T> {
    /// Resolve the instance serving the execution `ctx` belongs to, or the failure of a per-call
    /// target's startup hook, which fails the call. The hook's error is logged here, since the
    /// event's message leaves it out.
    pub async fn resolve(&self, ctx: Execution) -> Result<Arc<T>, HookFailed> {
        match self {
            Self::Singleton(instance) => Ok(instance.clone()),
            Self::PerCall(provider) => {
                // A live execution leaves the build nothing to refuse but a dependency the loader
                // checked resolving to another type, which fails the call the way a panicking
                // constructor does.
                let any = provider
                    .resolve(ctx)
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
                let built = *any
                    .downcast::<Result<Arc<T>, HookFailed>>()
                    .unwrap_or_else(|_| {
                        panic!(
                            "dispatch target '{}' resolved to a different type",
                            std::any::type_name::<T>()
                        )
                    });
                if let Err(failed) = &built {
                    tracing::error!(
                        dispatch_target = %failed.target,
                        hook = failed.hook,
                        error = %failed.source,
                        "a per-call dispatch target's startup hook failed; the call fails with it"
                    );
                }
                built
            }
        }
    }
}

/// The declared dependency tokens that are execution-scoped — the scan that decides
/// whether a dispatch target is built per call.
pub fn execution_scoped_dependencies(
    declared: &[String],
    dependencies: &FxHashMap<String, Arc<Box<dyn Provider>>>,
) -> Vec<String> {
    declared
        .iter()
        .filter(|token| {
            dependencies.get(*token).is_some_and(|provider| {
                matches!(provider.scope(), crate::di::ProviderScope::Execution)
            })
        })
        .cloned()
        .collect()
}
