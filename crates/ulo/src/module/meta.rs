use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::site::Sites;

/// Typed per-module metadata that a transport reads, written through
/// [`ModuleDef::meta`](crate::ModuleDef::meta). `ulo-http` stores its route middleware this way.
///
/// The value starts at `Default` and is configured in place. The sites it declares, such as the
/// middleware types it resolves, are checked by the wiring pass against the module's visibility.
/// A lazily loaded module that writes any metadata is refused with `LoadRefusal::Middleware`:
/// transports read it when they bind.
pub trait Meta: Default + Send + Sync + 'static {
    fn sites(&self, _s: &mut Sites) {}
}

/// The metadata values one module wrote, by type. Frozen into an `Arc` per value with the graph.
#[derive(Default)]
pub(crate) struct MetaMap {
    pub(crate) values: HashMap<TypeId, MetaEntry>,
}

pub(crate) struct MetaEntry {
    pub(crate) value: Box<dyn Any + Send + Sync>,
    /// `T::sites` over the value as written, captured at freeze.
    pub(crate) sites: fn(&(dyn Any + Send + Sync), &mut Sites),
}

impl MetaMap {
    pub(crate) fn get_or_default<T: Meta>(&mut self) -> &mut T {
        todo!()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// The frozen form a `ModuleRef` and a mounted handler read.
pub(crate) type FrozenMeta = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;
