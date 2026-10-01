//! The frozen, validated result of registering every module (§2, §10.1).
//!
//! `wire()` registers the root and everything it imports, freezes the records into a `Graph`,
//! and runs the six checks of §10.1, collecting every error. Nothing is built and nothing does
//! I/O. A `Graph` is immutable once frozen; `load` builds an extended copy and swaps it in.

pub(crate) mod cycles;
pub(crate) mod order;
pub(crate) mod register;
pub(crate) mod scopes;
pub(crate) mod visibility;
pub(crate) mod wire;

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::binding::{BindingRecord, Qualifier};
use crate::error::LookupError;
use crate::hooks::HookRecord;
use crate::key::Key;
use crate::module::meta::FrozenMeta;
use crate::module::{ModuleIdentity, ModuleName};
use crate::redact::SecretRegistry;
use crate::transport::controller::HandlerRecord;

/// A module's position in collection order: depth-first post-order over imports from the root,
/// imports in the order written. Lazily loaded modules follow, in load order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ModuleId(pub(crate) u32);

/// A binding's index in `Graph::bindings`. Ordering by id is ordering by
/// (module post-order index, declaration index), the tie-break of every order the core runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BindingId(pub(crate) u32);

pub(crate) struct Graph {
    pub(crate) modules: Vec<FrozenModule>,
    pub(crate) bindings: Vec<FrozenBinding>,
    /// By `ModuleId`: what each module sees.
    pub(crate) visibility: Vec<VisibilityTable>,
    /// Every contribution to each collection key, in collection order, from every module.
    pub(crate) collections: HashMap<Key, Arc<[BindingId]>>,
    pub(crate) inputs: HashMap<Key, InputDecl>,
    pub(crate) by_identity: HashMap<ModuleIdentity, ModuleId>,
    pub(crate) root: ModuleId,
    /// The singletons in the order `connect` builds them: a stable topological sort, the
    /// smallest `BindingId` among the ready ones first.
    pub(crate) connect_order: Vec<BindingId>,
    /// Every handler the controllers mounted, for every transport.
    pub(crate) handlers: Vec<HandlerRecord>,
    pub(crate) secrets: SecretRegistry,
}

pub(crate) struct FrozenModule {
    pub(crate) id: ModuleId,
    pub(crate) identity: ModuleIdentity,
    pub(crate) name: ModuleName,
    pub(crate) global: bool,
    pub(crate) keyed: Option<Qualifier>,
    pub(crate) imports: Vec<ModuleId>,
    pub(crate) bindings: Vec<BindingId>,
    /// Final export keys, requalified at a keyed module's boundary.
    pub(crate) exports: Vec<Key>,
    pub(crate) hooks: Vec<HookRecord>,
    pub(crate) meta: FrozenMeta,
    /// `Some(n)` for the n-th lazily loaded module; shutdown runs those in reverse load order.
    pub(crate) loaded: Option<u32>,
}

pub(crate) struct FrozenBinding {
    pub(crate) id: BindingId,
    pub(crate) origin: ModuleId,
    /// The record as registered; `record.keys()` yields the qualified keys.
    pub(crate) record: BindingRecord,
    pub(crate) role: Role,
    /// Decided by the scope pass (§6.2).
    pub(crate) effective: Effective,
    pub(crate) needs_execution: bool,
    /// One per site read, resolved against the origin module's visibility by step 3.
    pub(crate) edges: Vec<Edge>,
}

/// What `Auto` resolves against: a provider is a singleton; a controller or an enhancer is
/// inferred (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Provider,
    Controller,
    /// Referenced by type from an `EnhancerSpec`, or contributed under a role key.
    Enhancer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effective {
    Singleton,
    PerExecution,
    Transient,
}

pub(crate) struct Edge {
    pub(crate) target: EdgeTarget,
    /// Index into `record.sites.list`, for the site named in diagnostics.
    pub(crate) site: usize,
    pub(crate) optional: bool,
}

pub(crate) enum EdgeTarget {
    Binding(BindingId),
    Collection(Key),
    Input(Key),
    /// Reads the execution itself, or an extension: needs an execution, reaches no binding.
    Execution,
    Module,
}

/// What one module sees: its own bindings, its direct imports' exports and the globals'
/// exports. Collections are not in it; they are app-wide.
#[derive(Default)]
pub(crate) struct VisibilityTable {
    pub(crate) entries: HashMap<Key, Visible>,
}

pub(crate) enum Visible {
    Binding(BindingId),
    Input(Key),
    /// Two sources for one key: a wiring error for any site that reads it, naming every source.
    Ambiguous(Vec<(ModuleId, BindingId)>),
}

pub(crate) struct InputDecl {
    pub(crate) key: Key,
    pub(crate) seeder: TypeId,
    pub(crate) seeder_name: &'static str,
    pub(crate) declared_in: ModuleId,
}

impl Graph {
    pub(crate) fn binding(&self, id: BindingId) -> &FrozenBinding {
        &self.bindings[id.0 as usize]
    }

    pub(crate) fn module(&self, id: ModuleId) -> &FrozenModule {
        &self.modules[id.0 as usize]
    }

    /// The single binding or input `key` names from `module`. After wiring, a root lookup is
    /// never `Ambiguous`.
    pub(crate) fn lookup(&self, module: ModuleId, key: Key) -> Option<&Visible> {
        self.visibility[module.0 as usize].entries.get(&key)
    }

    /// Every contribution to `key` in collection order; empty when nothing contributes.
    pub(crate) fn collection(&self, key: Key) -> Arc<[BindingId]> {
        todo!()
    }

    /// The one registered module of type `ty` (the type a module's identity names) and
    /// qualifier: `LookupError::AmbiguousModule` over several configurations, `NotFound` with
    /// `LookupKind::Module` over none.
    pub(crate) fn find_module(&self, ty: TypeId, type_name: &'static str, qualifier: Option<Qualifier>) -> Result<ModuleId, LookupError> {
        todo!()
    }
}
