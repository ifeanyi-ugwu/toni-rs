use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// A typed per-message key-value bag, shared by everything handling that message.
///
/// Values are keyed by their concrete type. A guard that authenticates the
/// caller inserts an `AuthUser`; an interceptor, a pipe, or the handler itself
/// reads it back without coupling to the guard's type.
///
/// This is a handle, not the storage. Cloning it — or cloning the request parts
/// it travels on — yields another view of the same bag, which is what lets a
/// write from one pipeline stage reach a reader in a later one. Mutation goes
/// through `&self` for the same reason.
///
/// # Example
///
/// ```
/// use toni::context::Extensions;
///
/// #[derive(Clone)]
/// struct UserId(u64);
///
/// let ext = Extensions::new();
/// ext.insert(UserId(7));
///
/// // A clone reads the same bag.
/// assert_eq!(ext.clone().get::<UserId>().map(|u| u.0), Some(7));
/// ```
#[derive(Clone, Default)]
pub struct Extensions {
    inner: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

impl Extensions {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach a new bag to an HTTP request's own extension map and return the
    /// handle.
    ///
    /// Called once per request at the outermost seam, before the global
    /// middleware chain runs, so every stage — middleware, enhancers,
    /// providers, the handler — addresses one bag. Riding the request this way
    /// means a stage holding nothing but the request still reaches it.
    pub fn install(carrier: &mut http::Extensions) -> Self {
        let extensions = Self::new();
        carrier.insert(extensions.clone());
        extensions
    }

    /// Adopt the bag riding on an HTTP request, or mint a detached one when
    /// there is none.
    ///
    /// Read-only, so a bag minted here does not become the request's — use
    /// [`ensure`](Self::ensure) where the caller can write back.
    pub fn adopt(carrier: &http::Extensions) -> Self {
        carrier.get::<Self>().cloned().unwrap_or_else(Self::new)
    }

    /// Adopt the bag riding on an HTTP request, installing one if it has none.
    ///
    /// This is what a context constructor wants: a request that skipped the
    /// adapter seam — built directly in a test, or dispatched by a caller
    /// driving the pipeline itself — still ends up with the context and the
    /// request pointing at one bag, so a write from an enhancer reaches the
    /// handler on that path too.
    pub fn ensure(carrier: &mut http::Extensions) -> Self {
        match carrier.get::<Self>() {
            Some(existing) => existing.clone(),
            None => Self::install(carrier),
        }
    }

    /// Insert a value into the bag, returning the previous value of the same
    /// type, if any.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) -> Option<T> {
        self.inner
            .lock()
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|prev| prev.downcast::<T>().ok().map(|b| *b))
    }

    /// Returns a clone of the value of type `T`, if present.
    ///
    /// The bag is behind a lock, so a reference cannot escape it. Use
    /// [`with`](Self::with) for values that are expensive or impossible to clone.
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.inner
            .lock()
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
            .cloned()
    }

    /// Call `f` with the value of type `T`, if present, and return its result.
    ///
    /// The lock is held for the duration of `f` — don't reach back into the
    /// same bag from inside it.
    pub fn with<T: Send + Sync + 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.inner
            .lock()
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
            .map(f)
    }

    /// Call `f` with a mutable borrow of the value of type `T`, if present.
    ///
    /// Same locking caveat as [`with`](Self::with).
    pub fn with_mut<T: Send + Sync + 'static, R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.inner
            .lock()
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
            .map(f)
    }

    /// Remove and return the value of type `T`, if present.
    pub fn remove<T: Send + Sync + 'static>(&self) -> Option<T> {
        self.inner
            .lock()
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.inner.lock().contains_key(&TypeId::of::<T>())
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

/// Take the bag as a handler parameter to read what an earlier stage attached:
///
/// ```ignore
/// #[get("/me")]
/// fn me(&self, ext: Extensions) -> Body {
///     let user = ext.get::<Principal>().expect("AuthGuard runs first");
///     Body::text(user.name)
/// }
/// ```
///
/// Extraction never fails — a request with nothing attached yields an empty bag.
impl crate::extractors::FromRequestParts for Extensions {
    type Error = std::convert::Infallible;

    fn from_request_parts(parts: &crate::http_helpers::RequestPart) -> Result<Self, Self::Error> {
        Ok(Self::adopt(&parts.extensions))
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
        #[derive(Clone)]
        struct A(u32);
        #[derive(Clone)]
        struct B(&'static str);

        let ext = Extensions::new();
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
        #[derive(Clone)]
        struct A(u32);

        let ext = Extensions::new();
        assert!(ext.insert(A(1)).is_none());
        let prev = ext.insert(A(2)).unwrap();
        assert_eq!(prev.0, 1);
        assert_eq!(ext.get::<A>().unwrap().0, 2);
    }

    #[test]
    fn remove_returns_value() {
        #[derive(Clone)]
        struct A(u32);

        let ext = Extensions::new();
        ext.insert(A(7));
        let taken = ext.remove::<A>().unwrap();
        assert_eq!(taken.0, 7);
        assert!(ext.get::<A>().is_none());
    }

    #[test]
    fn with_mut_allows_in_place_update() {
        struct Counter(u32);

        let ext = Extensions::new();
        ext.insert(Counter(0));
        ext.with_mut::<Counter, _>(|c| c.0 += 5);
        assert_eq!(ext.with::<Counter, _>(|c| c.0), Some(5));
    }

    #[test]
    fn with_reads_a_value_that_cannot_be_cloned() {
        struct NoClone(String);

        let ext = Extensions::new();
        ext.insert(NoClone("held".into()));
        assert_eq!(ext.with::<NoClone, _>(|v| v.0.len()), Some(4));
    }

    #[test]
    fn contains_reports_presence() {
        struct A;
        struct B;

        let ext = Extensions::new();
        ext.insert(A);
        assert!(ext.contains::<A>());
        assert!(!ext.contains::<B>());
    }

    /// The whole point: a write through one handle is visible through another.
    #[test]
    fn clones_share_one_bag() {
        #[derive(Clone, PartialEq, Debug)]
        struct Principal(&'static str);

        let writer = Extensions::new();
        let reader = writer.clone();

        writer.insert(Principal("alice"));

        assert_eq!(reader.get::<Principal>(), Some(Principal("alice")));
    }
}
