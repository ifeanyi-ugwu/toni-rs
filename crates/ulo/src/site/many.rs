use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Scope};
use crate::site::{Site, SiteDesc};

/// Every contribution to `T @ Q`, in collection order, from every module in the application.
///
/// Collection order is depth-first post-order over imports from the root, imports in the order
/// written, then declaration order inside a module. Reading a `Many` constructs every
/// contribution; a transport walking a role collection reads it lazily through
/// [`Resolver::entries`] instead, so a later guard is never built when an earlier one refuses.
pub struct Many<T: ?Sized, Q = ()> {
    items: Arc<[Arc<T>]>,
    _q: PhantomData<fn() -> Q>,
}

impl<T: ?Sized, Q> Many<T, Q> {
    pub(crate) fn from_items(items: Arc<[Arc<T>]>) -> Self {
        Many { items, _q: PhantomData }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Arc<T>> {
        self.items.iter()
    }
}

impl<T: ?Sized, Q> Clone for Many<T, Q> {
    fn clone(&self) -> Self {
        Many { items: Arc::clone(&self.items), _q: PhantomData }
    }
}

impl<T: ?Sized, Q> Deref for Many<T, Q> {
    type Target = [Arc<T>];

    fn deref(&self) -> &[Arc<T>] {
        &self.items
    }
}

impl<'a, T: ?Sized, Q> IntoIterator for &'a Many<T, Q> {
    type Item = &'a Arc<T>;
    type IntoIter = std::slice::Iter<'a, Arc<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<T: ?Sized + Send + Sync + 'static, Q: 'static> Site for Many<T, Q> {
    fn describe(d: &mut SiteDesc) {
        d.many(Key::of::<T, Q>());
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        r.many_qualified::<T, Q>().await
    }
}

impl<T: ?Sized, Q, S: Scope> AllowedIn<S> for Many<T, Q> {}
