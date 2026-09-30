use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use parking_lot::Mutex;

/// Per-execution instance cache for execution-scoped providers.
///
/// One per execution, held by that execution's context. Ensures every
/// construction site — enhancer factories, the controller, any `#[new]`
/// constructor — resolves an execution-scoped type once and shares the result,
/// without a global registry.
///
/// Nothing in it is transport-specific. It lives on the context because that is
/// the object whose lifetime it shares: the execution's.
///
/// One `Arc` per type: [`insert`](Self::insert) keeps the first instance cached and
/// [`get`](Self::get) hands that `Arc` back, so every site in the execution holds one instance.
pub struct ExecutionCache {
    inner: Mutex<FxHashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// Instances keyed by the declaration that built them, for a declaration whose type another
    /// declaration may also build.
    keyed: Mutex<FxHashMap<String, Arc<dyn Any + Send + Sync>>>,
}

impl ExecutionCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FxHashMap::default()),
            keyed: Mutex::new(FxHashMap::default()),
        }
    }

    /// The instance cached under `key`, if one exists.
    pub(crate) fn get_keyed(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        self.keyed.lock().get(key).cloned()
    }

    /// Caches `value` under `key`, returning the instance cached first if another resolution got
    /// there before this one.
    pub(crate) fn insert_keyed(
        &self,
        key: &str,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Arc<dyn Any + Send + Sync> {
        self.keyed
            .lock()
            .entry(key.to_string())
            .or_insert(value)
            .clone()
    }

    /// The instance cached for `T`, if one exists.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let map = self.inner.lock();
        map.get(&TypeId::of::<T>())
            .cloned()
            .and_then(|v| v.downcast::<T>().ok())
    }

    /// Caches `value` for `T`, returning the instance cached first if another resolution got
    /// there before this one.
    pub fn insert<T: Any + Send + Sync>(&self, value: Arc<T>) -> Arc<T> {
        let cached = self
            .inner
            .lock()
            .entry(TypeId::of::<T>())
            .or_insert(value.clone() as Arc<dyn Any + Send + Sync>)
            .clone();
        cached.downcast::<T>().unwrap_or(value)
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

    #[derive(PartialEq, Debug)]
    struct Marker(u32);

    #[test]
    fn every_read_holds_the_first_instance_cached() {
        let cache = ExecutionCache::new();
        let first = cache.insert(Arc::new(Marker(7)));
        let second = cache.insert(Arc::new(Marker(8)));

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&first, &cache.get::<Marker>().unwrap()));
        assert_eq!(*first, Marker(7));
    }

    #[test]
    fn an_absent_type_is_none() {
        assert_eq!(ExecutionCache::new().get::<Marker>(), None);
    }
}
