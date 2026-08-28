use std::any::Any;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use super::provider::Provider;
use super::provider_context::ProviderContext;

/// How a dispatch target's instance is held: built once at startup and shared by every
/// call, or resolved per call from the target's own provider.
///
/// One value of this type sits behind every dispatch target — HTTP controller, RPC
/// controller, gRPC service — and [`instance`](DispatchSource::instance) is the one
/// resolution path. The transports differ only in where they call it and which
/// [`ProviderContext`] variant they pass.
pub enum DispatchSource<T> {
    /// Built at startup and shared by every call.
    Singleton(Arc<T>),
    /// The target's own provider, resolved inside the call being served.
    ///
    /// The provider must answer with `Arc<T>`, cache that `Arc` in the execution, and
    /// fire init/bootstrap at its own build site — hook resolution needs a
    /// concrete-type call site, which the generated provider body is and this generic
    /// code is not. A target asked for twice in one call is then built once and its
    /// hooks fire once.
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
    /// The instance serving the execution `ctx` belongs to.
    pub async fn instance(&self, ctx: ProviderContext) -> Arc<T> {
        match self {
            Self::Singleton(instance) => instance.clone(),
            Self::PerCall(provider) => {
                let any = provider.execute(vec![], ctx).await;
                *any.downcast::<Arc<T>>().unwrap_or_else(|_| {
                    panic!(
                        "dispatch target '{}' resolved to a different type",
                        std::any::type_name::<T>()
                    )
                })
            }
        }
    }
}

/// The declared dependency tokens that are request-scoped — the scan that decides
/// whether a dispatch target is built per call.
pub fn request_scoped_dependencies(
    declared: &[String],
    dependencies: &FxHashMap<String, Arc<Box<dyn Provider>>>,
) -> Vec<String> {
    declared
        .iter()
        .filter(|token| {
            dependencies.get(*token).is_some_and(|provider| {
                matches!(provider.get_scope(), crate::ProviderScope::Request)
            })
        })
        .cloned()
        .collect()
}
