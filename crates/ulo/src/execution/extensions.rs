use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::binding::{Instance, downcast_instance, instance_of};

/// The typed per-execution bag a guard or middleware writes and a handler or an
/// execution-scoped service reads as `Ext<T>`. Written through a shared reference, so every
/// clone of a transport's `Cx` writes the same bag.
#[derive(Default)]
pub struct Extensions {
    map: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Extensions {
    /// Stores `value`, replacing any earlier value of the same type.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) {
        self.map().insert(TypeId::of::<T>(), Arc::new(value));
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let value = self.map().get(&TypeId::of::<T>()).cloned()?;
        value.downcast::<T>().ok()
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.map().contains_key(&TypeId::of::<T>())
    }

    pub fn remove<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let value = self.map().remove(&TypeId::of::<T>())?;
        value.downcast::<T>().ok()
    }

    fn map(&self) -> MutexGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The execution inputs a transport seeded, by type. Read as `Dep<T>` through the input's key.
///
/// Each input is held as an [`Instance`], the form every stored value takes, so a lookup by an
/// erased key reads it with `downcast_instance` like any binding's instance.
#[derive(Default)]
pub(crate) struct Inputs {
    map: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Inputs {
    pub(crate) fn insert<T: Send + Sync + 'static>(&self, value: T) {
        self.map().insert(TypeId::of::<T>(), instance_of(Arc::new(value)));
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let instance: Instance = self.map().get(&TypeId::of::<T>()).cloned()?;
        downcast_instance::<T>(&instance)
    }

    /// The input under `ty` erased, for a lookup by key: an [`Instance`] wrapping the `Arc<T>`.
    pub(crate) fn get_erased(&self, ty: TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        self.map().get(&ty).cloned()
    }

    fn map(&self) -> MutexGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
