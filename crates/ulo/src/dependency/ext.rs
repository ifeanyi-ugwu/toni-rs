use std::ops::Deref;
use std::sync::Arc;

use crate::dependency::{FromContainer, Requirement};
use crate::error::LookupError;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Auto, PerExecution, Transient};

/// A typed view of per-execution data: the extension `T` a guard or middleware wrote earlier in
/// this execution through [`Extensions::insert`](crate::Extensions::insert).
///
/// Needs an execution, so an explicit singleton cannot read one. An extension nothing has
/// written fails with `LookupError::NotFound`; `Option<Ext<T>>` reads it as `None`.
pub struct Ext<T>(Arc<T>);

impl<T> Ext<T> {
    pub(crate) fn from_arc(inner: Arc<T>) -> Self {
        Ext(inner)
    }
}

impl<T> Clone for Ext<T> {
    fn clone(&self) -> Self {
        Ext(Arc::clone(&self.0))
    }
}

impl<T> Deref for Ext<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Send + Sync + 'static> FromContainer for Ext<T> {
    fn describe(req: &mut Requirement) {
        req.ext::<T>();
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        r.ext::<T>()
    }
}

impl<T> AllowedIn<PerExecution> for Ext<T> {}
impl<T> AllowedIn<Transient> for Ext<T> {}
impl<T> AllowedIn<Auto> for Ext<T> {}
