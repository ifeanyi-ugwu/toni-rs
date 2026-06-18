use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A typed per-request key-value bag.
///
/// Values are keyed by their concrete type. Use [`insert`](Self::insert) to
/// attach data to the request and [`get`](Self::get) / [`get_mut`](Self::get_mut)
/// to read it back.
///
/// Modeled after `http::Extensions`, but does not require `Clone` — values move
/// in and out by ownership. Use this for cross-cutting concerns: a guard that
/// authenticates a user inserts an `AuthUser`; a downstream interceptor reads
/// it without coupling to the guard's type.
///
/// # Example
///
/// ```
/// use toni::context::Extensions;
///
/// struct UserId(u64);
///
/// let mut ext = Extensions::new();
/// ext.insert(UserId(7));
/// assert_eq!(ext.get::<UserId>().map(|u| u.0), Some(7));
/// ```
#[derive(Default)]
pub struct Extensions {
    map: Option<Box<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

impl Extensions {
    pub fn new() -> Self {
        Self { map: None }
    }

    /// Insert a value into the bag, returning the previous value of the same
    /// type, if any.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .get_or_insert_with(|| Box::new(HashMap::new()))
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|prev| prev.downcast::<T>().ok().map(|b| *b))
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .as_ref()?
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .as_mut()?
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Remove and return the value of type `T`, if present.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map
            .as_mut()?
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.map
            .as_ref()
            .map(|m| m.contains_key(&TypeId::of::<T>()))
            .unwrap_or(false)
    }

    pub fn clear(&mut self) {
        if let Some(m) = self.map.as_mut() {
            m.clear();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.as_ref().map(|m| m.is_empty()).unwrap_or(true)
    }

    pub fn len(&self) -> usize {
        self.map.as_ref().map(|m| m.len()).unwrap_or(0)
    }
}

impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        struct A(u32);
        struct B(&'static str);

        let mut ext = Extensions::new();
        assert!(ext.is_empty());

        ext.insert(A(1));
        ext.insert(B("hi"));
        assert_eq!(ext.len(), 2);
        assert_eq!(ext.get::<A>().unwrap().0, 1);
        assert_eq!(ext.get::<B>().unwrap().0, "hi");
        assert!(ext.get::<u8>().is_none());
    }

    #[test]
    fn insert_returns_previous() {
        struct A(u32);

        let mut ext = Extensions::new();
        assert!(ext.insert(A(1)).is_none());
        let prev = ext.insert(A(2)).unwrap();
        assert_eq!(prev.0, 1);
        assert_eq!(ext.get::<A>().unwrap().0, 2);
    }

    #[test]
    fn remove_returns_value() {
        struct A(u32);

        let mut ext = Extensions::new();
        ext.insert(A(7));
        let taken = ext.remove::<A>().unwrap();
        assert_eq!(taken.0, 7);
        assert!(ext.get::<A>().is_none());
    }

    #[test]
    fn get_mut_allows_in_place_update() {
        struct Counter(u32);

        let mut ext = Extensions::new();
        ext.insert(Counter(0));
        ext.get_mut::<Counter>().unwrap().0 += 5;
        assert_eq!(ext.get::<Counter>().unwrap().0, 5);
    }

    #[test]
    fn contains_reports_presence() {
        struct A;
        struct B;

        let mut ext = Extensions::new();
        ext.insert(A);
        assert!(ext.contains::<A>());
        assert!(!ext.contains::<B>());
    }
}
