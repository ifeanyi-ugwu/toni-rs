use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::sync::Arc;

use crate::dependency::Dependencies;

/// Typed per-module metadata that a transport reads, written through
/// [`ModuleDef::meta`](crate::ModuleDef::meta). `ulo-http` stores its route middleware this way.
///
/// The value starts at `Default` and is configured in place. The dependencies it declares, such
/// as the middleware types it resolves, are checked by the wiring pass against the module's
/// visibility. A lazily loaded module that writes any metadata is refused with
/// `LoadRefusal::Middleware`: transports read it when they bind.
pub trait Meta: Default + Send + Sync + 'static {
    fn dependencies(&self, _d: &mut Dependencies) {}
}

/// The metadata values one module wrote, by type. Frozen into an `Arc` per value with the graph.
#[derive(Default)]
pub(crate) struct MetaMap {
    pub(crate) values: HashMap<TypeId, MetaEntry>,
}

pub(crate) struct MetaEntry {
    pub(crate) value: Box<dyn Any + Send + Sync>,
    /// `T::dependencies` over the value as written, captured at freeze.
    pub(crate) dependencies: fn(&(dyn Any + Send + Sync), &mut Dependencies),
    /// `type_name::<T>()`, for a wiring error naming the metadata whose dependency is missing.
    pub(crate) name: &'static str,
}

impl MetaMap {
    pub(crate) fn get_or_default<T: Meta>(&mut self) -> &mut T {
        let entry = self.values.entry(TypeId::of::<T>()).or_insert_with(|| MetaEntry {
            value: Box::new(T::default()),
            dependencies: meta_dependencies::<T>,
            name: type_name::<T>(),
        });
        entry.value.downcast_mut::<T>().expect("a metadata value is stored under its own TypeId")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

fn meta_dependencies<T: Meta>(value: &(dyn Any + Send + Sync), d: &mut Dependencies) {
    if let Some(value) = value.downcast_ref::<T>() {
        value.dependencies(d);
    }
}

/// The frozen form a `ModuleRef` and a mounted handler read.
pub(crate) type FrozenMeta = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;
