use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use parking_lot::Mutex;

/// Per-execution instance cache for request-scoped providers.
///
/// One per execution, held by that execution's context. Ensures every
/// construction site — enhancer factories, the controller, any `#[new]`
/// constructor — resolves a request-scoped type once and shares the result,
/// without a global registry.
///
/// Nothing in it is transport-specific. It lives on the context because that is
/// the object whose lifetime it shares: the execution's.
///
/// # Instances are shared by clone
///
/// [`get`](Self::get) hands back a clone, and injected fields bind an owned
/// value. The cache therefore guarantees *one construction* per request, not
/// one live value: two injection sites hold two copies that were built once.
/// A request-scoped provider whose state must be visible across sites has to
/// carry that state behind a shared handle (`Arc<Mutex<_>>`, `Arc<OnceLock<_>>`)
/// — mutating a plain field mutates only that site's copy.
pub struct ExecutionCache {
    inner: Mutex<FxHashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl ExecutionCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FxHashMap::default()),
        }
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

impl Default for ExecutionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct Marker(u32);

    #[test]
    fn one_construction_is_shared_by_clone() {
        let cache = ExecutionCache::new();
        cache.insert(Marker(7));

        assert_eq!(cache.get::<Marker>(), Some(Marker(7)));
        assert_eq!(cache.get::<Marker>(), Some(Marker(7)));
    }

    #[test]
    fn an_absent_type_is_none() {
        assert_eq!(ExecutionCache::new().get::<Marker>(), None);
    }
}
