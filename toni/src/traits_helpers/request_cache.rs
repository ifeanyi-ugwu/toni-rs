use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use parking_lot::Mutex;

use crate::http_helpers::RequestPart;

/// Per-request instance cache for request-scoped providers.
///
/// Installed once at the start of each request and threaded through all
/// `Provider::execute` calls via `ProviderContext::Http`. Ensures that every
/// construction site in a request — enhancer factories, the controller, and
/// any `#[new]` constructor — resolves a request-scoped type once and shares
/// the result, without a global registry.
///
/// The cache travels on the request parts (see [`install`](Self::install) /
/// [`adopt`](Self::adopt)) rather than being passed explicitly, so a
/// construction site holding only `&RequestPart` joins the same request.
///
/// # Instances are shared by clone
///
/// [`get`](Self::get) hands back a clone, and injected fields bind an owned
/// value. The cache therefore guarantees *one construction* per request, not
/// one live value: two injection sites hold two copies that were built once.
/// A request-scoped provider whose state must be visible across sites has to
/// carry that state behind a shared handle (`Arc<Mutex<_>>`, `Arc<OnceLock<_>>`)
/// — mutating a plain field mutates only that site's copy.
pub struct RequestCache {
    inner: Mutex<FxHashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl RequestCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FxHashMap::default()),
        }
    }

    /// Attach a new cache to `parts` and return it.
    ///
    /// Called once per request at the head of the route pipeline. Every later
    /// construction site reaches this same cache through [`adopt`](Self::adopt),
    /// including sites that only ever see a clone of the parts.
    ///
    /// Callers driving the container directly — tests, CLI entry points,
    /// background jobs — can install a cache on their own parts to place
    /// several `resolve` calls in one request scope.
    pub fn install(parts: &mut RequestPart) -> Arc<Self> {
        let cache = Arc::new(Self::new());
        parts.extensions.insert(cache.clone());
        cache
    }

    /// Adopt the cache carried on `parts`.
    ///
    /// Falls back to a fresh detached cache when `parts` is absent or carries
    /// none — construction outside a request pipeline still works, it just
    /// shares nothing.
    pub fn adopt(parts: Option<&RequestPart>) -> Arc<Self> {
        parts
            .and_then(|p| p.extensions.get::<Arc<Self>>())
            .cloned()
            .unwrap_or_else(|| Arc::new(Self::new()))
    }

    /// Returns a clone of the cached instance for `T`, if one exists.
    pub fn get<T: Any + Clone + Send + Sync>(&self) -> Option<T> {
        let map = self.inner.lock();
        map.get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }

    /// Stores `value` in the cache under `T`'s `TypeId`.
    pub fn insert<T: Any + Clone + Send + Sync>(&self, value: T) {
        let mut map = self.inner.lock();
        map.insert(
            TypeId::of::<T>(),
            Arc::new(value) as Arc<dyn Any + Send + Sync>,
        );
    }
}

impl Default for RequestCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct Marker(u32);

    fn parts() -> RequestPart {
        http::Request::builder().body(()).unwrap().into_parts().0
    }

    #[test]
    fn adopt_returns_the_installed_cache() {
        let mut parts = parts();
        let installed = RequestCache::install(&mut parts);
        installed.insert(Marker(7));

        assert_eq!(
            RequestCache::adopt(Some(&parts)).get::<Marker>(),
            Some(Marker(7))
        );
    }

    /// The parts are cloned on the way to the handler, so the clone has to
    /// reach the same cache — not a copy of it.
    #[test]
    fn a_cloned_parts_adopts_the_same_cache() {
        let mut parts = parts();
        RequestCache::install(&mut parts);
        let cloned = parts.clone();

        RequestCache::adopt(Some(&parts)).insert(Marker(1));
        assert_eq!(
            RequestCache::adopt(Some(&cloned)).get::<Marker>(),
            Some(Marker(1))
        );
    }

    #[test]
    fn adopt_without_an_installed_cache_is_detached() {
        let parts = parts();
        RequestCache::adopt(Some(&parts)).insert(Marker(3));

        assert_eq!(RequestCache::adopt(Some(&parts)).get::<Marker>(), None);
        assert_eq!(RequestCache::adopt(None).get::<Marker>(), None);
    }
}
