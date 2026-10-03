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

use crate::binding::{BindingRecord, Qualifier, Recipe};
use crate::dependency::{Dependencies, DependencyLabel, DependencyRecord, ReadKind};
use crate::error::{LookupError, LookupKind};
use crate::hooks::HookRecord;
use crate::key::{BindingKind, Key, KeyName, short_type_name};
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

/// A clone shares every closure, value and constructor with the original, which is how a lazy
/// load extends a copy while executions keep reading the graph they started on.
#[derive(Clone)]
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
    /// Every singleton in the order `connect` builds them: a stable topological sort, the
    /// smallest `BindingId` among the ready ones first. Values are in it, so the store holds
    /// them and their readiness checks and hooks run; aliases are not, being read through their
    /// target.
    pub(crate) connect_order: Vec<BindingId>,
    /// Every handler the controllers mounted, for every transport.
    pub(crate) handlers: Vec<HandlerRecord>,
    pub(crate) secrets: SecretRegistry,
    /// By `ModuleId`: each module's exports as an importer sees them, requalified at a keyed
    /// boundary and resolved to their binding. Freezing drops the export records, and a lazily
    /// loaded module importing a frozen one reads its exports from here.
    pub(in crate::graph) exported: Vec<Vec<(Key, BindingId)>>,
}

#[derive(Clone)]
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

#[derive(Clone)]
pub(crate) struct FrozenBinding {
    pub(crate) id: BindingId,
    pub(crate) origin: ModuleId,
    /// The record as registered; `record.keys()` yields the qualified keys.
    pub(crate) record: BindingRecord,
    pub(crate) role: Role,
    /// Decided by the scope pass (§6.2).
    pub(crate) effective: Effective,
    pub(crate) needs_execution: bool,
    /// One per dependency read, resolved against the origin module's visibility by step 3. A
    /// read that resolved to nothing has no edge: an optional read that found no binding, or a
    /// missing or ambiguous key already reported. An alias has no dependencies and so no edges;
    /// its target is `Recipe::Alias { target }`, looked up in the origin module's table.
    pub(crate) edges: Vec<Edge>,
}

/// What `Auto` resolves against: a provider is a singleton; a controller or an enhancer is
/// inferred (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Provider,
    Controller,
    /// Referenced by type from an `EnhancerSpec`, or contributed through `ModuleDef::enhancer`.
    /// A contribution through `contribute` is a provider whatever its key.
    Enhancer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effective {
    Singleton,
    PerExecution,
    Transient,
}

#[derive(Clone)]
pub(crate) struct Edge {
    pub(crate) target: EdgeTarget,
    /// Index into `record.dependencies.list`, for the injection point named in diagnostics.
    pub(crate) dependency: usize,
    pub(crate) optional: bool,
}

#[derive(Clone)]
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
///
/// Every key of a binding maps to the same `BindingId`: its primary key, each `also_as` key, and
/// each export key requalified at a keyed boundary. A reader picks the coercion by type, since a
/// requalified key's qualifier is not the record's.
#[derive(Clone, Default)]
pub(crate) struct VisibilityTable {
    pub(crate) entries: HashMap<Key, Visible>,
}

#[derive(Clone)]
pub(crate) enum Visible {
    Binding(BindingId),
    Input(Key),
    /// Two sources for one key: a wiring error for any read of it, naming every source.
    /// One binding reached by two routes, an import's export and its re-export by another
    /// import, is one source. The root's table holds none once wiring passes.
    Ambiguous(Vec<(ModuleId, BindingId)>),
}

#[derive(Clone)]
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
        match self.collections.get(&key) {
            Some(ids) => Arc::clone(ids),
            None => Arc::from(Vec::new()),
        }
    }

    /// The one registered module of type `ty` (the type a module's identity names) and
    /// qualifier: `LookupError::AmbiguousModule` over several configurations, `NotFound` with
    /// `LookupKind::Module` over none. `None` matches every instance of the type, keyed or not,
    /// so a type imported bare and keyed, or under two keys, is ambiguous without a qualifier.
    pub(crate) fn find_module(&self, ty: TypeId, type_name: &'static str, qualifier: Option<Qualifier>) -> Result<ModuleId, LookupError> {
        let found: Vec<&FrozenModule> = self
            .modules
            .iter()
            .filter(|m| m.identity.type_id() == ty && (qualifier.is_none() || m.identity.qualifier() == qualifier))
            .collect();
        match found.as_slice() {
            [one] => Ok(one.id),
            [] => {
                let q = qualifier.unwrap_or_else(Qualifier::none);
                Err(LookupError::NotFound {
                    key: Key::from_parts(ty, type_name, q.id, q.name).name(BindingKind::Single),
                    kind: LookupKind::Module,
                })
            }
            several => Err(LookupError::AmbiguousModule {
                module: type_name,
                candidates: several.iter().map(|m| m.name.clone()).collect(),
            }),
        }
    }

    pub(in crate::graph) fn empty() -> Graph {
        Graph {
            modules: Vec::new(),
            bindings: Vec::new(),
            visibility: Vec::new(),
            collections: HashMap::new(),
            inputs: HashMap::new(),
            by_identity: HashMap::new(),
            root: ModuleId(0),
            connect_order: Vec::new(),
            handlers: Vec::new(),
            secrets: SecretRegistry::default(),
            exported: Vec::new(),
        }
    }

    pub(in crate::graph) fn module_name(&self, id: ModuleId) -> ModuleName {
        self.module(id).name.clone()
    }

    /// A binding's qualified primary key, as the errors name it.
    pub(in crate::graph) fn key_name(&self, id: BindingId) -> KeyName {
        let record = &self.binding(id).record;
        record_key(record).name(record.kind)
    }

    /// A binding as a step of a printed path: the type it builds, with its qualifier.
    pub(in crate::graph) fn label(&self, id: BindingId) -> String {
        let record = &self.binding(id).record;
        let mut text = short_type_name(record.built);
        if record.qualifier != Qualifier::none() {
            text.push_str(" @ ");
            text.push_str(&short_type_name(record.qualifier.name));
        }
        text
    }

    /// `label`, followed by the scope the pass decided when it is not a singleton:
    /// `AuditContext (execution)`.
    pub(in crate::graph) fn scoped_label(&self, id: BindingId) -> String {
        let mut text = self.label(id);
        match self.binding(id).effective {
            Effective::PerExecution => text.push_str(" (execution)"),
            Effective::Transient => text.push_str(" (transient)"),
            Effective::Singleton => {}
        }
        text
    }

    /// What reads injection point `index` of binding `id`, as a missing-dependency report names
    /// it: ``UserService (param `mailer`)``, or ``PgPool factory (param #1)`` for a factory.
    pub(in crate::graph) fn consumer(&self, id: BindingId, index: usize) -> String {
        let record = &self.binding(id).record;
        let mut text = self.label(id);
        if matches!(record.recipe, Recipe::Factory(_)) && !record.constructs {
            text.push_str(" factory");
        }
        match record.dependencies.list.get(index) {
            Some(dependency) => format!("{text} ({})", dependency_label(dependency.label)),
            None => text,
        }
    }

    /// The binding an alias reads, one step: `None` for anything else, or a target the alias's
    /// module does not see as a single binding.
    pub(in crate::graph) fn alias_target(&self, id: BindingId) -> Option<BindingId> {
        let binding = self.binding(id);
        let Recipe::Alias { target } = &binding.record.recipe else { return None };
        match self.lookup(binding.origin, *target) {
            Some(Visible::Binding(target)) => Some(*target),
            _ => None,
        }
    }

    /// What building `id` reads: the bindings its edges reach, every contribution of each
    /// collection it reads, and an alias's target.
    pub(in crate::graph) fn construction_deps(&self, id: BindingId) -> Vec<BindingId> {
        let mut deps = Vec::new();
        for edge in &self.binding(id).edges {
            match &edge.target {
                EdgeTarget::Binding(dep) => deps.push(*dep),
                EdgeTarget::Collection(key) => deps.extend(self.collections.get(key).into_iter().flat_map(|ids| ids.iter().copied())),
                EdgeTarget::Input(_) | EdgeTarget::Execution | EdgeTarget::Module => {}
            }
        }
        deps.extend(self.alias_target(id));
        deps
    }

    /// The bindings `dependencies` read when resolved in `module`, for closures the graph keeps no
    /// edges for: readiness checks and hooks.
    pub(in crate::graph) fn closure_deps(&self, module: ModuleId, dependencies: &Dependencies) -> Vec<BindingId> {
        let mut deps = Vec::new();
        for read in dependencies.list.iter().flat_map(|d| &d.requirement.reads) {
            match &read.kind {
                ReadKind::Single(key) => {
                    if let Some(Visible::Binding(dep)) = self.lookup(module, *key) {
                        deps.push(*dep);
                    }
                }
                ReadKind::Collection(key) => {
                    deps.extend(self.collections.get(key).into_iter().flat_map(|ids| ids.iter().copied()));
                }
                ReadKind::Extension(_) | ReadKind::Execution | ReadKind::Module => {}
            }
        }
        deps
    }

    /// What `id`'s readiness check reads besides `id` itself: the check runs right after `id` is
    /// built and before anything that depends on it, so these come first.
    pub(in crate::graph) fn readiness_deps(&self, id: BindingId) -> Vec<BindingId> {
        let binding = self.binding(id);
        match &binding.record.ready {
            Some(ready) => self.closure_deps(binding.origin, &ready.dependencies).into_iter().filter(|dep| *dep != id).collect(),
            None => Vec::new(),
        }
    }

    /// Whether `id` hands a need for an execution to whatever reads it: an execution-scoped
    /// binding always does, a transient one when it needs one itself.
    pub(in crate::graph) fn passes_execution(&self, id: BindingId) -> bool {
        let binding = self.binding(id);
        match binding.effective {
            Effective::PerExecution => true,
            Effective::Transient => binding.needs_execution,
            Effective::Singleton => false,
        }
    }

    /// The first edge of `id` that reads execution data directly: an extension, the execution,
    /// or an execution input.
    pub(in crate::graph) fn direct_execution_edge(&self, id: BindingId) -> Option<&Edge> {
        self.binding(id)
            .edges
            .iter()
            .find(|edge| matches!(edge.target, EdgeTarget::Execution | EdgeTarget::Input(_)))
    }

    /// A handler as the reports name it: `UsersController::get_rpc`.
    pub(in crate::graph) fn handler_name(&self, handler: &HandlerRecord) -> String {
        format!("{}::{}", self.label(handler.controller), handler.decl.name)
    }

    /// Injection point `index` of binding `id` as the last step of a printed path:
    /// ``Dep<RequestHead> (field `head`)``.
    pub(in crate::graph) fn dependency_step(&self, id: BindingId, index: usize) -> String {
        match self.binding(id).record.dependencies.list.get(index) {
            Some(dependency) => dependency_text(dependency),
            None => String::new(),
        }
    }
}

/// A record's primary key with its qualifier applied.
pub(in crate::graph) fn record_key(record: &BindingRecord) -> Key {
    record.primary.with_qualifier(record.qualifier.id, record.qualifier.name)
}

/// An export key as it leaves a module: an unqualified key leaving a keyed module becomes
/// `T @ Q`; anything else leaves as written (§8.3).
pub(in crate::graph) fn boundary_key(keyed: Option<Qualifier>, key: Key) -> Key {
    match keyed {
        Some(q) if key.is_unqualified() => key.with_qualifier(q.id, q.name),
        _ => key,
    }
}

/// ``field `name` ``, ``param `name` ``, or `param #n` for a closure's n-th parameter.
pub(in crate::graph) fn dependency_label(label: DependencyLabel) -> String {
    match label {
        DependencyLabel::Field(name) => format!("field `{name}`"),
        DependencyLabel::Param(name) => format!("param `{name}`"),
        DependencyLabel::Position(index) => format!("param #{}", index + 1),
    }
}

/// An injection point's type and label: ``Dep<RequestHead> (field `head`)``.
pub(in crate::graph) fn dependency_text(dependency: &DependencyRecord) -> String {
    format!("{} ({})", short_type_name(dependency.type_name), dependency_label(dependency.label))
}
