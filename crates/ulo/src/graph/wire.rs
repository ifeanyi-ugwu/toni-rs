//! The wiring pass: the six steps of §10.1, collecting every error and stopping at none. Steps
//! that depend on a missing piece skip only the affected edges, so one missing binding does not
//! hide unrelated errors.

use std::any::type_name;
use std::collections::{HashMap, HashSet};
use std::panic::Location;
use std::sync::Arc;

use crate::binding::{AlsoAs, BindingRecord, Qualifier, ReadyRecord, Recipe, instance_of};
use crate::error::LoadRefusal;
use crate::error::wiring::{WiringError, WiringErrors};
use crate::graph::register::{Import, Registry};
use crate::graph::{
    BindingId, Edge, EdgeTarget, Effective, FrozenBinding, FrozenModule, Graph, InputDecl, ModuleId, Role, Visible,
    VisibilityTable, boundary_key, cycles, order, record_key, register, scopes, visibility,
};
use crate::hooks::HookRecord;
use crate::key::{BindingKind, Key, KeyName};
use crate::module::def::{ExportRecord, InputRecord, ModuleNode};
use crate::module::meta::FrozenMeta;
use crate::module::{Module, ModuleIdentity, ModuleName};
use crate::redact::redact;
use crate::scope::ScopeKind;
use crate::site::{ReadKind, SiteDesc, SiteRead, SiteRecord, Sites};
use crate::testing::{CollectionOverride, Override, OverrideTarget, TestPlan};
use crate::timer::{Bound, Timer};
use crate::transport::controller::{EnhancerDep, HandlerDecl, HandlerRecord, Mount};

/// What the wiring pass needs to know about the app beyond its modules.
pub(crate) struct WireEnv {
    /// When set, bound under `dyn Timer` as a value in the core's own global module.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    /// The builder knobs that were set: each is a wiring error without a `Timer` (step 6).
    pub(crate) knobs_set: Vec<&'static str>,
}

/// Registers the root, freezes the graph and runs every check. `plan` carries a test's
/// overrides and module replacements.
pub(crate) fn wire(root: Box<dyn Module>, env: &WireEnv, plan: Option<TestPlan>) -> Result<Graph, WiringErrors> {
    let mut registry = register::register_all(root, plan.as_ref());
    if let Some(timer) = &env.timer {
        add_timer_module(&mut registry, timer);
    }
    let mut graph = Graph::empty();
    let mut steps = Steps::default();
    let declared = freeze(&mut graph, registry, None, plan, &mut steps);
    graph.root = declared.root;
    check(&mut graph, &declared, env, &mut steps);

    let errors = steps.into_errors();
    if !errors.is_empty() {
        return Err(WiringErrors::new(errors));
    }
    graph.connect_order = order::connect_order(&graph);
    Ok(graph)
}

/// A lazily loaded module wired against the frozen graph: every error collected, and every
/// refusal of §8.6 checked before any of it is built.
///
/// The base graph is copied, never changed: executions still reading it keep a consistent
/// snapshot. An identity the base already holds answers with its module and no new singleton.
pub(crate) fn wire_lazy(base: &Graph, module: Box<dyn Module>, env: &WireEnv) -> Result<LazyWiring, LazyFailure> {
    if let Some(&existing) = base.by_identity.get(&module.identity()) {
        return Ok(LazyWiring { graph: clone_graph(base), module: existing, singletons: Vec::new() });
    }
    let registry = register::register_lazy(base, module);
    if let Some(refusal) = refusal(base, &registry) {
        return Err(LazyFailure::Refused(refusal));
    }

    let mut graph = clone_graph(base);
    let load = base.modules.iter().filter_map(|m| m.loaded).max().map_or(0, |last| last + 1);
    let mut steps = Steps::default();
    let declared = freeze(&mut graph, registry, Some(load), None, &mut steps);
    check(&mut graph, &declared, env, &mut steps);

    let errors = steps.into_errors();
    if !errors.is_empty() {
        return Err(LazyFailure::Wiring(WiringErrors::new(errors)));
    }
    // Base singletons never wait on new ones, so the base order is a prefix of the new one and
    // what follows it is exactly what the load brought.
    graph.connect_order = order::connect_order(&graph);
    let singletons =
        graph.connect_order.iter().copied().filter(|id| id.0 as usize >= declared.first_binding).collect();
    Ok(LazyWiring { graph, module: declared.root, singletons })
}

pub(crate) struct LazyWiring {
    /// The base graph extended with the module and the imports it brought. Every base module and
    /// binding keeps its id; the new modules are the ids from the base's module count up, in
    /// collection order, each `loaded` with one number, greater than any earlier load's.
    pub(crate) graph: Graph,
    pub(crate) module: ModuleId,
    /// The new singletons in connect order, for `load` to build and check.
    pub(crate) singletons: Vec<BindingId>,
}

pub(crate) enum LazyFailure {
    Wiring(WiringErrors),
    Refused(LoadRefusal),
}

/// What the checks read that the frozen graph does not keep: the records as written, for the
/// modules this wiring added.
pub(crate) struct Declared {
    /// The module the registration started from.
    pub(crate) root: ModuleId,
    /// The first module, binding and handler this wiring added; a lazy load's base graph holds
    /// everything before them.
    pub(crate) first_module: usize,
    pub(crate) first_binding: usize,
    pub(crate) first_handler: usize,
    pub(crate) exports: Vec<(ModuleId, ExportRecord)>,
    pub(crate) inputs: Vec<(ModuleId, InputRecord)>,
    /// Each metadata value's sites, with the metadata type's name.
    pub(crate) meta: Vec<(ModuleId, &'static str, Sites)>,
}

/// The errors of one run by step, so the report lists them in the order of §10.1 whatever order
/// the passes find them in.
#[derive(Default)]
struct Steps {
    modules: Vec<WiringError>,
    bindings: Vec<WiringError>,
    sites: Vec<WiringError>,
    cycles: Vec<WiringError>,
    scopes: Vec<WiringError>,
    environment: Vec<WiringError>,
}

impl Steps {
    fn into_errors(self) -> Vec<WiringError> {
        let mut errors = self.modules;
        errors.extend(self.bindings);
        errors.extend(self.sites);
        errors.extend(self.cycles);
        errors.extend(self.scopes);
        errors.extend(self.environment);
        errors
    }
}

/// Steps 1 to 6 over a frozen graph. The tables come first: re-exports, aliases and every site
/// resolve against them. Roles come before the scope pass, which reads them; the cycle check
/// runs on the resolved edges before either.
fn check(graph: &mut Graph, declared: &Declared, env: &WireEnv, steps: &mut Steps) {
    visibility::build_tables(graph, declared);
    check_modules(graph, declared, &mut steps.modules);
    check_bindings(graph, declared, &mut steps.bindings);
    visibility::resolve_sites(graph, declared, &mut steps.sites);
    cycles::dependency_cycles(graph, &mut steps.cycles);
    scopes::assign_roles(graph);
    scopes::needs_execution(graph);
    scopes::check_scopes(graph, &mut steps.scopes);
    scopes::check_inputs(graph, &mut steps.scopes);
    check_environment(graph, env, declared, &mut steps.environment);
}

/// The core's own global module, holding the app's `Timer` under `dyn Timer` (§3.9).
struct TimerModule;

/// First in collection order: it imports nothing, and every module sees it as a global.
fn add_timer_module(registry: &mut Registry, timer: &Arc<dyn Timer>) {
    let identity = ModuleIdentity::of_type::<TimerModule>().label("AppBuilder::timer");
    let mut node = ModuleNode::new(identity.clone());
    node.global = true;
    let key = Key::of::<dyn Timer, ()>();
    let location = Location::caller();
    node.bindings.push(BindingRecord::new(
        key,
        type_name::<dyn Timer>(),
        BindingKind::Single,
        ScopeKind::Singleton,
        Recipe::Value(instance_of::<dyn Timer>(Arc::clone(timer))),
        Sites::default(),
        location,
    ));
    node.exports.push(ExportRecord { key, reexport: false, location });
    let index = registry.nodes.len();
    registry.nodes.push(node);
    registry.by_identity.insert(identity, index);
    registry.imports.push(Vec::new());
    registry.post_order.insert(0, index);
}

/// Appends the registry's modules to `graph` in collection order, assigning ids, requalifying
/// keyed exports, mounting controllers, and freezing metadata. Along the way it reports import
/// cycles (step 1) and, for step 2, the `try_value` failures (redacted), a replacement's missing
/// exports, and the test plan's overrides as it applies them. `load` numbers the modules of a
/// lazy load.
fn freeze(
    graph: &mut Graph,
    mut registry: Registry,
    load: Option<u32>,
    plan: Option<TestPlan>,
    steps: &mut Steps,
) -> Declared {
    let names = registry.names(Some(&*graph));
    cycles::import_cycles(&registry, &names, &mut steps.modules);

    for text in registry.nodes.iter().flat_map(|node| &node.secrets) {
        if !text.is_empty() {
            graph.secrets.register(text.clone());
        }
    }

    for replaced in &registry.replaced {
        let Some(node) = registry.nodes.get(replaced.replacement) else { continue };
        let offered: HashSet<Key> = node.exports.iter().map(|export| boundary_key(node.keyed, export.key)).collect();
        let missing: Vec<KeyName> =
            replaced.exports.iter().filter(|key| !offered.contains(*key)).map(|key| key.name(BindingKind::Single)).collect();
        if !missing.is_empty() {
            steps.bindings.push(WiringError::ReplacementMissingExports {
                original: ModuleName::of(&replaced.original, 0),
                replacement: names[replaced.replacement].clone(),
                missing,
            });
        }
    }

    // Paired by order before any override runs: only `try_value` writes a `Failed` record, and
    // it records the failure in the same call. An override may then replace a failed recipe,
    // and a failure whose record no longer fails is not reported.
    let mut failures = Vec::new();
    for (index, node) in registry.nodes.iter_mut().enumerate() {
        let failed: Vec<usize> = node
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, record)| matches!(record.recipe, Recipe::Failed))
            .map(|(position, _)| position)
            .collect();
        for (position, failure) in failed.into_iter().zip(node.failures.drain(..)) {
            failures.push((index, position, failure));
        }
    }

    let (overrides, collections) = match plan {
        Some(TestPlan { overrides, collections, .. }) => (overrides, collections),
        None => (Vec::new(), Vec::new()),
    };
    apply_overrides(&mut registry, &names, overrides, &mut steps.bindings);

    for (index, position, failure) in failures {
        let Some(record) = registry.nodes[index].bindings.get(position) else { continue };
        if !matches!(record.recipe, Recipe::Failed) {
            continue;
        }
        steps.bindings.push(WiringError::ValueFailed {
            module: names[index].clone(),
            key: record_key(record).name(BindingKind::Single),
            error: redact(&graph.secrets, failure.error),
            at: failure.location,
        });
    }

    // After the failures are paired: removing contributions moves the records after them.
    apply_collections(&mut registry, collections, &mut steps.bindings);

    let first_module = graph.modules.len();
    let mut module_of = vec![ModuleId(first_module as u32); registry.nodes.len()];
    for (position, &node) in registry.post_order.iter().enumerate() {
        module_of[node] = ModuleId((first_module + position) as u32);
    }
    let mut declared = Declared {
        root: module_of.first().copied().unwrap_or(ModuleId(first_module as u32)),
        first_module,
        first_binding: graph.bindings.len(),
        first_handler: graph.handlers.len(),
        exports: Vec::new(),
        inputs: Vec::new(),
        meta: Vec::new(),
    };

    let mut contributions: HashMap<Key, Vec<BindingId>> = HashMap::new();
    let mut nodes: Vec<Option<ModuleNode>> = registry.nodes.into_iter().map(Some).collect();
    for (position, &index) in registry.post_order.iter().enumerate() {
        let Some(node) = nodes[index].take() else { continue };
        let id = ModuleId((first_module + position) as u32);
        let ModuleNode { identity, global, keyed, bindings, exports, controllers, inputs, hooks, meta, .. } = node;

        let mut binding_ids = Vec::with_capacity(bindings.len());
        for record in bindings {
            let binding = BindingId(graph.bindings.len() as u32);
            if record.kind == BindingKind::Collection {
                contributions.entry(record_key(&record)).or_default().push(binding);
            }
            let role = if record.controller { Role::Controller } else { Role::Provider };
            graph.bindings.push(FrozenBinding {
                id: binding,
                origin: id,
                record,
                role,
                effective: Effective::Singleton,
                needs_execution: false,
                edges: Vec::new(),
            });
            binding_ids.push(binding);
        }

        for controller in controllers {
            let Some(&binding) = binding_ids.get(controller.binding) else { continue };
            let mut decls = Vec::new();
            (controller.mount)(&mut Mount { handlers: &mut decls });
            graph.handlers.extend(decls.into_iter().map(|decl| HandlerRecord { controller: binding, module: id, decl }));
        }

        for input in inputs {
            graph.inputs.entry(input.key).or_insert(InputDecl {
                key: input.key,
                seeder: input.seeder,
                seeder_name: input.seeder_name,
                declared_in: id,
            });
            declared.inputs.push((id, input));
        }

        let mut frozen_meta = FrozenMeta::new();
        let mut entries: Vec<_> = meta.values.into_iter().collect();
        entries.sort_by_key(|(_, entry)| entry.name);
        for (ty, entry) in entries {
            let mut sites = Sites::default();
            (entry.sites)(&*entry.value, &mut sites);
            declared.meta.push((id, entry.name, sites));
            frozen_meta.insert(ty, Arc::from(entry.value));
        }

        let export_keys: Vec<Key> = exports.iter().map(|export| boundary_key(keyed, export.key)).collect();
        declared.exports.extend(exports.into_iter().map(|export| (id, export)));
        let imports: Vec<ModuleId> = registry.imports[index]
            .iter()
            .map(|import| match *import {
                Import::New(node) => module_of[node],
                Import::Existing(module) => module,
            })
            .collect();

        graph.by_identity.insert(identity.clone(), id);
        graph.modules.push(FrozenModule {
            id,
            identity,
            name: names[index].clone(),
            global,
            keyed,
            imports,
            bindings: binding_ids,
            exports: export_keys,
            hooks,
            meta: frozen_meta,
            loaded: load,
        });
    }

    for (key, ids) in contributions {
        let mut merged: Vec<BindingId> = graph.collections.get(&key).map(|existing| existing.to_vec()).unwrap_or_default();
        merged.extend(ids);
        graph.collections.insert(key, Arc::from(merged));
    }
    declared
}

/// Step 2, tests: each override replaces the recipe and sites of the bindings it matches, which
/// keep their origin module, scope, visibility, exports, hooks and readiness check. An override
/// of an `also_as` key splits that key off into a binding of its own in the same module, since
/// the override's value is not the type the original builds.
fn apply_overrides(registry: &mut Registry, names: &[ModuleName], overrides: Vec<Override>, errors: &mut Vec<WiringError>) {
    let timer = Key::of::<dyn Timer, ()>();
    for ov in overrides {
        if ov.key == timer {
            errors.push(WiringError::TimerOverride { at: ov.location });
            continue;
        }
        let modules: Vec<usize> = match &ov.target {
            OverrideTarget::Unscoped | OverrideTarget::Everywhere => registry.post_order.clone(),
            OverrideTarget::ModuleType { ty, name } => match one_instance(registry, names, *ty, None, *name, &ov, errors) {
                Some(module) => vec![module],
                None => continue,
            },
            OverrideTarget::ModuleKeyed { ty, name, qualifier } => {
                match one_instance(registry, names, *ty, Some(*qualifier), *name, &ov, errors) {
                    Some(module) => vec![module],
                    None => continue,
                }
            }
            OverrideTarget::Module(identity) => match registry.by_identity.get(identity) {
                Some(&module) => vec![module],
                None => {
                    errors.push(WiringError::OverrideUnmatched { key: ov.key.name(BindingKind::Single), at: ov.location });
                    continue;
                }
            },
        };

        let matches: Vec<(usize, usize)> = modules
            .iter()
            .flat_map(|&module| {
                registry.nodes[module]
                    .bindings
                    .iter()
                    .enumerate()
                    .filter(|(_, record)| record.kind == BindingKind::Single && record.keys().any(|key| key == ov.key))
                    .map(move |(position, _)| (module, position))
            })
            .collect();

        if matches.is_empty() {
            let collection = modules.iter().any(|&module| {
                registry.nodes[module]
                    .bindings
                    .iter()
                    .any(|record| record.kind == BindingKind::Collection && record_key(record) == ov.key)
            });
            errors.push(if collection {
                WiringError::OverrideKind {
                    key: ov.key.name(BindingKind::Single),
                    expected: BindingKind::Collection,
                    found: BindingKind::Single,
                    at: ov.location,
                }
            } else {
                WiringError::OverrideUnmatched { key: ov.key.name(BindingKind::Single), at: ov.location }
            });
            continue;
        }
        if matches.len() > 1 && matches!(ov.target, OverrideTarget::Unscoped) {
            let mut matched: Vec<ModuleName> = matches.iter().map(|&(module, _)| names[module].clone()).collect();
            matched.dedup();
            errors.push(WiringError::OverrideAmbiguous { key: ov.key.name(BindingKind::Single), matches: matched, at: ov.location });
            continue;
        }
        for (module, position) in matches {
            replace_recipe(&mut registry.nodes[module], position, &ov);
        }
    }
}

/// The one module of type `ty` (and qualifier, when given) an override is scoped to: unmatched
/// over none, `OverrideModuleAmbiguous` over several configured or keyed instances.
fn one_instance(
    registry: &Registry,
    names: &[ModuleName],
    ty: std::any::TypeId,
    qualifier: Option<Qualifier>,
    name: &'static str,
    ov: &Override,
    errors: &mut Vec<WiringError>,
) -> Option<usize> {
    let found: Vec<usize> = registry
        .post_order
        .iter()
        .copied()
        .filter(|&node| {
            let identity = &registry.nodes[node].identity;
            identity.type_id() == ty && (qualifier.is_none() || identity.qualifier() == qualifier)
        })
        .collect();
    match found.as_slice() {
        [one] => Some(*one),
        [] => {
            errors.push(WiringError::OverrideUnmatched { key: ov.key.name(BindingKind::Single), at: ov.location });
            None
        }
        several => {
            errors.push(WiringError::OverrideModuleAmbiguous {
                module: name,
                candidates: several.iter().map(|&node| names[node].clone()).collect(),
                at: ov.location,
            });
            None
        }
    }
}

fn replace_recipe(node: &mut ModuleNode, position: usize, ov: &Override) {
    let Some(record) = node.bindings.get_mut(position) else { return };
    if record_key(record) == ov.key {
        record.recipe = clone_recipe(&ov.recipe);
        record.sites = clone_sites(&ov.sites);
        return;
    }
    let qualifier = record.qualifier;
    let scope = record.scope;
    record.also.retain(|also| also.key.with_qualifier(qualifier.id, qualifier.name) != ov.key);
    let mut split = BindingRecord::new(
        ov.key,
        ov.key.type_name(),
        BindingKind::Single,
        scope,
        clone_recipe(&ov.recipe),
        clone_sites(&ov.sites),
        ov.location,
    );
    split.qualifier = qualifier;
    node.bindings.push(split);
}

/// Step 2, tests: `override_many` removes every contribution to its key, from every module, and
/// contributes its items in their place, in the order given, from the root module.
fn apply_collections(registry: &mut Registry, collections: Vec<CollectionOverride>, errors: &mut Vec<WiringError>) {
    for co in collections {
        let removed: usize = registry.nodes.iter_mut().map(|node| remove_contributions(node, co.key)).sum();
        if removed == 0 {
            let single = registry
                .nodes
                .iter()
                .any(|node| node.bindings.iter().any(|r| r.kind == BindingKind::Single && r.keys().any(|key| key == co.key)));
            errors.push(if single {
                WiringError::OverrideKind {
                    key: co.key.name(BindingKind::Collection),
                    expected: BindingKind::Single,
                    found: BindingKind::Collection,
                    at: co.location,
                }
            } else {
                WiringError::OverrideUnmatched { key: co.key.name(BindingKind::Collection), at: co.location }
            });
            continue;
        }
        let Some(root) = registry.nodes.first_mut() else { continue };
        for item in co.items {
            // The item is already an `Arc` of the collection's type, so it needs no
            // `into_primary`.
            let mut record = BindingRecord::new(
                co.key,
                co.key.type_name(),
                BindingKind::Collection,
                ScopeKind::Singleton,
                Recipe::Value(item),
                Sites::default(),
                co.location,
            );
            record.qualifier = qualifier_of(co.key);
            root.bindings.push(record);
        }
    }
}

/// Removes `node`'s contributions to `key`, keeping each controller pointed at its own record.
fn remove_contributions(node: &mut ModuleNode, key: Key) -> usize {
    let mut removed_before = Vec::with_capacity(node.bindings.len());
    let mut kept = Vec::with_capacity(node.bindings.len());
    let mut removed = 0;
    for record in node.bindings.drain(..) {
        removed_before.push(removed);
        if record.kind == BindingKind::Collection && record_key(&record) == key {
            removed += 1;
        } else {
            kept.push(record);
        }
    }
    node.bindings = kept;
    for controller in &mut node.controllers {
        if let Some(&shift) = removed_before.get(controller.binding) {
            controller.binding -= shift;
        }
    }
    removed
}

fn qualifier_of(key: Key) -> Qualifier {
    let none = Qualifier::none();
    Qualifier { id: key.qualifier_id(), name: key.qualifier_name().unwrap_or(none.name) }
}

/// Step 1: re-exports a module cannot see unambiguously, exports of keys a module does not bind,
/// and inputs declared inside a keyed module. Import cycles are reported by `freeze`, which
/// still has the registry.
fn check_modules(graph: &Graph, declared: &Declared, errors: &mut Vec<WiringError>) {
    for (module, export) in &declared.exports {
        let key = export.key.name(BindingKind::Single);
        if export.reexport {
            match graph.lookup(*module, export.key) {
                Some(Visible::Binding(_)) => {}
                Some(Visible::Ambiguous(list)) => errors.push(WiringError::ReexportAmbiguous {
                    module: graph.module_name(*module),
                    key,
                    sources: list.iter().map(|&(source, _)| graph.module_name(source)).collect(),
                    at: export.location,
                }),
                Some(Visible::Input(_)) | None => {
                    errors.push(WiringError::ReexportNotVisible { module: graph.module_name(*module), key, at: export.location })
                }
            }
        } else if visibility::own_binding(graph, *module, export.key).is_none() {
            let consumer = if graph.lookup(*module, export.key).is_some() {
                "its export list (a key an import provides leaves through `reexport`)"
            } else {
                "its export list"
            };
            errors.push(WiringError::Missing {
                key,
                consumer: consumer.to_owned(),
                module: graph.module_name(*module),
                near: visibility::near_spelling(graph, *module, export.key)
                    .map(|(near, exporter)| (near.name(BindingKind::Single), graph.module_name(exporter))),
            });
        }
    }
    for (module, input) in &declared.inputs {
        if graph.module(*module).keyed.is_some() {
            errors.push(WiringError::KeyedInput {
                module: graph.module_name(*module),
                key: input.key.name(BindingKind::Single),
                at: input.location,
            });
        }
    }
}

/// Step 2: duplicate single bindings, single/collection mixes, aliases pointing at nothing, two
/// readiness checks on one binding, and an execution input declared twice or also bound.
fn check_bindings(graph: &Graph, declared: &Declared, errors: &mut Vec<WiringError>) {
    for module in graph.modules.iter().skip(declared.first_module) {
        let mut first: HashMap<Key, BindingId> = HashMap::new();
        let mut pairs: HashSet<(BindingId, BindingId)> = HashSet::new();
        for &id in &module.bindings {
            let record = &graph.binding(id).record;
            if record.kind != BindingKind::Single {
                continue;
            }
            for key in record.keys() {
                match first.get(&key) {
                    Some(&earlier) if earlier != id => {
                        if pairs.insert((earlier, id)) {
                            errors.push(WiringError::DuplicateBinding {
                                key: key.name(BindingKind::Single),
                                module: module.name.clone(),
                                first: graph.binding(earlier).record.location,
                                second: record.location,
                            });
                        }
                    }
                    Some(_) => {}
                    None => {
                        first.insert(key, id);
                    }
                }
            }
        }
    }

    // Collections are app-wide, so a single binding anywhere under a collection's key mixes the
    // two kinds. Reported once per key, where either side is new to this wiring.
    let mut mixed: HashSet<Key> = HashSet::new();
    for binding in &graph.bindings {
        if binding.record.kind != BindingKind::Single {
            continue;
        }
        for key in binding.record.keys() {
            let Some(contributions) = graph.collections.get(&key) else { continue };
            let new = binding.id.0 as usize >= declared.first_binding
                || contributions.iter().any(|id| id.0 as usize >= declared.first_binding);
            if new && mixed.insert(key) {
                if let Some(&first) = contributions.first() {
                    errors.push(WiringError::KindMix {
                        key: key.name(BindingKind::Single),
                        module: graph.module_name(binding.origin),
                        single: binding.record.location,
                        collection: graph.binding(first).record.location,
                    });
                }
            }
        }
    }

    for binding in graph.bindings.iter().skip(declared.first_binding) {
        let record = &binding.record;
        if let Recipe::Alias { target } = &record.recipe {
            match graph.lookup(binding.origin, *target) {
                Some(Visible::Binding(_)) => {}
                Some(Visible::Ambiguous(list)) => errors.push(WiringError::Ambiguous {
                    key: target.name(BindingKind::Single),
                    consumer: format!("the alias `{}`", graph.key_name(binding.id)),
                    module: graph.module_name(binding.origin),
                    sources: list
                        .iter()
                        .map(|&(source, id)| (graph.module_name(source), graph.binding(id).record.location))
                        .collect(),
                }),
                Some(Visible::Input(_)) | None => errors.push(WiringError::DanglingAlias {
                    alias: graph.key_name(binding.id),
                    target: target.name(BindingKind::Single),
                    module: graph.module_name(binding.origin),
                    at: record.location,
                }),
            }
        }
        if let Some(&first) = record.replaced_ready.first() {
            let second = record.replaced_ready.get(1).copied().or_else(|| record.ready.as_ref().map(|ready| ready.location));
            if let Some(second) = second {
                errors.push(WiringError::DuplicateReadiness {
                    key: graph.key_name(binding.id),
                    module: graph.module_name(binding.origin),
                    first,
                    second,
                });
            }
        }
    }

    let mut first_input: HashMap<Key, &'static Location<'static>> = HashMap::new();
    for (module, input) in &declared.inputs {
        let key = input.key.name(BindingKind::Single);
        match first_input.get(&input.key) {
            Some(&first) => errors.push(WiringError::DuplicateBinding {
                key,
                module: graph.module_name(*module),
                first,
                second: input.location,
            }),
            None => {
                first_input.insert(input.key, input.location);
            }
        }
        for binding in &graph.bindings {
            if binding.record.kind == BindingKind::Single && binding.record.keys().any(|bound| bound == input.key) {
                errors.push(WiringError::DuplicateBinding {
                    key,
                    module: graph.module_name(binding.origin),
                    first: input.location,
                    second: binding.record.location,
                });
            }
        }
    }
}

/// Step 6: a `Timer` wherever an explicit bound is written, a readiness `.timeout` or
/// `.attempt_timeout`, a hook's or constructor's `After(..)`, or a builder knob. A lazy load
/// checks what it brought; the knobs were checked when the app wired.
fn check_environment(graph: &Graph, env: &WireEnv, declared: &Declared, errors: &mut Vec<WiringError>) {
    if env.timer.is_some() {
        return;
    }
    if declared.first_module == 0 {
        errors.extend(env.knobs_set.iter().map(|&knob| WiringError::KnobWithoutTimer { knob }));
    }
    for binding in graph.bindings.iter().skip(declared.first_binding) {
        let record = &binding.record;
        let name = graph.label(binding.id);
        if matches!(record.construct_bound, Bound::After(_)) {
            errors.push(WiringError::BoundWithoutTimer { item: format!("construction of `{name}`"), at: record.location });
        }
        if let Some(ready) = &record.ready {
            if matches!(ready.whole, Bound::After(_)) {
                errors.push(WiringError::BoundWithoutTimer {
                    item: format!("readiness `.timeout` of `{name}`"),
                    at: ready.location,
                });
            }
            if matches!(ready.attempt, Bound::After(_)) {
                errors.push(WiringError::BoundWithoutTimer {
                    item: format!("readiness `.attempt_timeout` of `{name}`"),
                    at: ready.location,
                });
            }
        }
        for hook in &record.hooks {
            if matches!(hook.bound, Bound::After(_)) {
                errors.push(WiringError::BoundWithoutTimer {
                    item: format!("`{}` hook of `{name}`", hook.kind),
                    at: hook.location,
                });
            }
        }
    }
    for module in graph.modules.iter().skip(declared.first_module) {
        for hook in &module.hooks {
            if matches!(hook.bound, Bound::After(_)) {
                errors.push(WiringError::BoundWithoutTimer {
                    item: format!("`{}` hook of module {}", hook.kind, module.name),
                    at: hook.location,
                });
            }
        }
    }
}

/// The first thing §8.6 refuses in a lazy load, checked over every module it brought, in
/// collection order: controllers, metadata, inputs, global exports, then contributions to a
/// collection the base graph already has or reads.
fn refusal(base: &Graph, registry: &Registry) -> Option<LoadRefusal> {
    let names = registry.names(Some(base));
    for &index in &registry.post_order {
        let node = &registry.nodes[index];
        let module = || names[index].clone();
        if !node.controllers.is_empty() {
            return Some(LoadRefusal::Controllers { module: module() });
        }
        if !node.meta.is_empty() {
            return Some(LoadRefusal::Middleware { module: module() });
        }
        if let Some(input) = node.inputs.first() {
            return Some(LoadRefusal::Input { module: module(), key: input.key.name(BindingKind::Single) });
        }
        if node.global {
            if let Some(export) = node.exports.first() {
                let key = boundary_key(node.keyed, export.key).name(BindingKind::Single);
                return Some(LoadRefusal::GlobalExport { module: module(), key });
            }
        }
        for record in node.bindings.iter().filter(|record| record.kind == BindingKind::Collection) {
            let key = record_key(record);
            if base.collections.contains_key(&key) || reads_collection(base, key) {
                return Some(LoadRefusal::Contribution { module: module(), key: key.name(BindingKind::Collection) });
            }
        }
    }
    None
}

/// Whether anything in `base` reads `key` as a collection: a binding's site, a readiness check
/// or hook closure, a module hook, an enhancer closure, or a transport's role collection.
fn reads_collection(base: &Graph, key: Key) -> bool {
    let in_sites = |sites: &Sites| {
        sites.list.iter().flat_map(|site| &site.desc.reads).any(|read| matches!(&read.kind, ReadKind::Collection(k) if *k == key))
    };
    base.bindings.iter().any(|binding| {
        binding.edges.iter().any(|edge| matches!(&edge.target, EdgeTarget::Collection(k) if *k == key))
            || binding.record.ready.as_ref().is_some_and(|ready| in_sites(&ready.sites))
            || binding.record.hooks.iter().any(|hook| in_sites(&hook.sites))
    }) || base.modules.iter().any(|module| module.hooks.iter().any(|hook| in_sites(&hook.sites)))
        || base.handlers.iter().any(|handler| {
            handler.decl.role_keys.contains(&key)
                || handler.decl.enhancer_deps.iter().any(|dep| match dep {
                    EnhancerDep::Closure(sites) => in_sites(&**sites),
                    EnhancerDep::Type(_) => false,
                })
        })
}

/// A deep copy of a frozen graph, for a lazy load to extend. The records hold `Arc`s to their
/// closures and values, so the copy shares every instance and constructor with the original.
fn clone_graph(base: &Graph) -> Graph {
    Graph {
        modules: base.modules.iter().map(clone_module).collect(),
        bindings: base.bindings.iter().map(clone_binding).collect(),
        visibility: base.visibility.iter().map(clone_table).collect(),
        collections: base.collections.clone(),
        inputs: base
            .inputs
            .iter()
            .map(|(key, decl)| {
                let decl = InputDecl { key: decl.key, seeder: decl.seeder, seeder_name: decl.seeder_name, declared_in: decl.declared_in };
                (*key, decl)
            })
            .collect(),
        by_identity: base.by_identity.clone(),
        root: base.root,
        connect_order: base.connect_order.clone(),
        handlers: base.handlers.iter().map(clone_handler).collect(),
        secrets: base.secrets.clone(),
        exported: base.exported.clone(),
    }
}

fn clone_module(module: &FrozenModule) -> FrozenModule {
    FrozenModule {
        id: module.id,
        identity: module.identity.clone(),
        name: module.name.clone(),
        global: module.global,
        keyed: module.keyed,
        imports: module.imports.clone(),
        bindings: module.bindings.clone(),
        exports: module.exports.clone(),
        hooks: module.hooks.iter().map(clone_hook).collect(),
        meta: module.meta.clone(),
        loaded: module.loaded,
    }
}

fn clone_binding(binding: &FrozenBinding) -> FrozenBinding {
    FrozenBinding {
        id: binding.id,
        origin: binding.origin,
        record: clone_record(&binding.record),
        role: binding.role,
        effective: binding.effective,
        needs_execution: binding.needs_execution,
        edges: binding.edges.iter().map(clone_edge).collect(),
    }
}

fn clone_edge(edge: &Edge) -> Edge {
    let target = match &edge.target {
        EdgeTarget::Binding(id) => EdgeTarget::Binding(*id),
        EdgeTarget::Collection(key) => EdgeTarget::Collection(*key),
        EdgeTarget::Input(key) => EdgeTarget::Input(*key),
        EdgeTarget::Execution => EdgeTarget::Execution,
        EdgeTarget::Module => EdgeTarget::Module,
    };
    Edge { target, site: edge.site, optional: edge.optional }
}

fn clone_table(table: &VisibilityTable) -> VisibilityTable {
    let entries = table
        .entries
        .iter()
        .map(|(key, visible)| {
            let visible = match visible {
                Visible::Binding(id) => Visible::Binding(*id),
                Visible::Input(input) => Visible::Input(*input),
                Visible::Ambiguous(list) => Visible::Ambiguous(list.clone()),
            };
            (*key, visible)
        })
        .collect();
    VisibilityTable { entries }
}

fn clone_handler(handler: &HandlerRecord) -> HandlerRecord {
    let decl = &handler.decl;
    HandlerRecord {
        controller: handler.controller,
        module: handler.module,
        decl: HandlerDecl {
            transport: decl.transport,
            transport_name: decl.transport_name,
            name: decl.name,
            role_keys: decl.role_keys,
            enhancer_deps: decl
                .enhancer_deps
                .iter()
                .map(|dep| match dep {
                    EnhancerDep::Type(key) => EnhancerDep::Type(*key),
                    EnhancerDep::Closure(sites) => EnhancerDep::Closure(Arc::clone(sites)),
                })
                .collect(),
            specs: Arc::clone(&decl.specs),
            handler: Arc::clone(&decl.handler),
        },
    }
}

fn clone_record(record: &BindingRecord) -> BindingRecord {
    BindingRecord {
        primary: record.primary,
        qualifier: record.qualifier,
        built: record.built,
        into_primary: record.into_primary.clone(),
        also: record.also.iter().map(|also| AlsoAs { key: also.key, coerce: Arc::clone(&also.coerce) }).collect(),
        kind: record.kind,
        scope: record.scope,
        controller: record.controller,
        recipe: clone_recipe(&record.recipe),
        sites: clone_sites(&record.sites),
        construct_bound: record.construct_bound,
        ready: record.ready.as_ref().map(clone_ready),
        replaced_ready: record.replaced_ready.clone(),
        hooks: record.hooks.iter().map(clone_hook).collect(),
        constructs: record.constructs,
        location: record.location,
    }
}

fn clone_ready(ready: &ReadyRecord) -> ReadyRecord {
    ReadyRecord {
        check: Arc::clone(&ready.check),
        sites: clone_sites(&ready.sites),
        retries: ready.retries,
        backoff: ready.backoff,
        whole: ready.whole,
        attempt: ready.attempt,
        location: ready.location,
    }
}

fn clone_hook(hook: &HookRecord) -> HookRecord {
    HookRecord { kind: hook.kind, bound: hook.bound, run: Arc::clone(&hook.run), sites: clone_sites(&hook.sites), location: hook.location }
}

fn clone_recipe(recipe: &Recipe) -> Recipe {
    match recipe {
        Recipe::Construct(ctor) => Recipe::Construct(Arc::clone(ctor)),
        Recipe::Factory(ctor) => Recipe::Factory(Arc::clone(ctor)),
        Recipe::Value(instance) => Recipe::Value(Arc::clone(instance)),
        Recipe::Alias { target } => Recipe::Alias { target: *target },
        Recipe::Failed => Recipe::Failed,
    }
}

fn clone_sites(sites: &Sites) -> Sites {
    let list = sites
        .list
        .iter()
        .map(|site| SiteRecord {
            label: site.label,
            type_name: site.type_name,
            desc: SiteDesc {
                reads: site.desc.reads.iter().map(|read| SiteRead { kind: clone_read(&read.kind), optional: read.optional }).collect(),
            },
        })
        .collect();
    Sites { list }
}

fn clone_read(kind: &ReadKind) -> ReadKind {
    match kind {
        ReadKind::Single(key) => ReadKind::Single(*key),
        ReadKind::Collection(key) => ReadKind::Collection(*key),
        ReadKind::Extension(key) => ReadKind::Extension(*key),
        ReadKind::Execution => ReadKind::Execution,
        ReadKind::Module => ReadKind::Module,
    }
}
