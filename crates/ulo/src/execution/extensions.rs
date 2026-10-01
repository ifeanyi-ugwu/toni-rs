use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
        todo!()
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        todo!()
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        todo!()
    }

    pub fn remove<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        todo!()
    }
}

/// The execution inputs a transport seeded, by type. Read as `Dep<T>` through the input's key.
#[derive(Default)]
pub(crate) struct Inputs {
    map: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Inputs {
    pub(crate) fn insert<T: Send + Sync + 'static>(&self, value: T) {
        todo!()
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        todo!()
    }

    /// The input under `ty` erased, for a lookup by key.
    pub(crate) fn get_erased(&self, ty: TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        todo!()
    }
}
