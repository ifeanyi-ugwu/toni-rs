use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use crate::dependency::{FromContainer, Requirement};
use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Scope};

/// The single binding `T @ Q`, as the shared `Arc` every holder of the binding holds.
///
/// Services are never cloned; `Dep` clones the pointer, and mutation goes through interior
/// mutability, so whatever an init hook does is seen by every holder. Reading a `Dep<T>`
/// requires `T: Send + Sync`: for a trait object that means `Send + Sync` as supertraits of the
/// trait, since `dyn Repo` and `dyn Repo + Send + Sync` are distinct keys.
pub struct Dep<T: ?Sized, Q = ()> {
    inner: Arc<T>,
    _q: PhantomData<fn() -> Q>,
}

impl<T: ?Sized, Q> Dep<T, Q> {
    pub fn from_arc(inner: Arc<T>) -> Self {
        Dep { inner, _q: PhantomData }
    }

    pub fn into_arc(self) -> Arc<T> {
        self.inner
    }

    pub fn arc(&self) -> &Arc<T> {
        &self.inner
    }
}

impl<T: ?Sized, Q> Clone for Dep<T, Q> {
    fn clone(&self) -> Self {
        Dep { inner: Arc::clone(&self.inner), _q: PhantomData }
    }
}

impl<T: ?Sized, Q> Deref for Dep<T, Q> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized + Send + Sync + 'static, Q: 'static> FromContainer for Dep<T, Q> {
    fn describe(req: &mut Requirement) {
        req.dep(Key::of::<T, Q>());
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        r.dep_qualified::<T, Q>().await
    }
}

/// Whether the binding behind a `Dep` needs an execution is decided at `wire()`, not here.
impl<T: ?Sized, Q, S: Scope> AllowedIn<S> for Dep<T, Q> {}
