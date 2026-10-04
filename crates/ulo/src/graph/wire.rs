//! The wiring pass: the six steps of §10.1, collecting every error and stopping at none. Steps
//! that depend on a missing piece skip only the affected edges, so one missing binding does not
//! hide unrelated errors.

use std::any::type_name;
use std::collections::{HashMap, HashSet};
use std::panic::Location;
use std::sync::Arc;

use crate::binding::{BindingRecord, Qualifier, Recipe, instance_of};
use crate::dependency::{Dependencies, ReadKind};
use crate::error::LoadRefusal;
use crate::error::wiring::{InputOrigin, WiringError, WiringErrors};
use crate::graph::register::{Import, Registry};
use crate::graph::{
    BindingId, EdgeTarget, Effective, FrozenBinding, FrozenModule, Graph, InputDecl, ModuleId, Role, Visible,
    boundary_key, cycles, order, record_key, register, scopes, visibility,
};
use crate::key::{BindingKind, Key, KeyName};
use crate::module::def::{ExportRecord, InputRecord, ModuleNode};
use crate::module::meta::FrozenMeta;
use crate::module::{Module, ModuleIdentity, ModuleName};
use crate::redact::redact;
use crate::scope::ScopeKind;
use crate::testing::{CollectionOverride, Override, OverrideTarget, Replacement, TestPlan};
use crate::timer::{Bound, Timer};
use crate::transport::controller::{EnhancerDep, HandlerDecl, HandlerRecord, Mount};
use crate::transport::inputs::Inputs;
use crate::type_name::TypeName;

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
        return Ok(LazyWiring { graph: base.clone(), module: existing, singletons: Vec::new() });
    }
    let registry = register::register_lazy(base, module);
    if let Some(refusal) = refusal(base, &registry) {
        return Err(LazyFailure::Refused(refusal));
    }

    let mut graph = base.clone();
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
    /// Every input a `Transport::inputs` declared, in mount order, each with its transport origin.
    /// A lazy load mounts no handler and declares none.
    pub(crate) transport_inputs: Vec<InputDecl>,
    /// Each metadata value's dependencies, with the metadata type's name.
    pub(crate) meta: Vec<(ModuleId, &'static str, Dependencies)>,
}

/// The errors of one run by step, so the report lists them in the order of §10.1 whatever order
/// the passes find them in.
#[derive(Default)]
struct Steps {
    modules: Vec<WiringError>,
    bindings: Vec<WiringError>,
    dependencies: Vec<WiringError>,
    cycles: Vec<WiringError>,
    scopes: Vec<WiringError>,
    environment: Vec<WiringError>,
}

impl Steps {
    fn into_errors(self) -> Vec<WiringError> {
        let mut errors = self.modules;
        errors.extend(self.bindings);
        errors.extend(self.dependencies);
        errors.extend(self.cycles);
        errors.extend(self.scopes);
        errors.extend(self.environment);
        errors
    }
}

/// Steps 1 to 6 over a frozen graph. The tables come first: re-exports, aliases and every
/// injection point resolve against them. Roles come before the scope pass, which reads them; the
/// cycle check runs on the resolved edges before either. The scopes of enhancers declared by
/// closure and the closure check read the scopes the pass decided.
fn check(graph: &mut Graph, declared: &Declared, env: &WireEnv, steps: &mut Steps) {
    visibility::build_tables(graph, declared);
    check_modules(graph, declared, &mut steps.modules);
    check_bindings(graph, declared, &mut steps.bindings);
    visibility::resolve_dependencies(graph, declared, &mut steps.dependencies);
    cycles::dependency_cycles(graph, &mut steps.cycles);
    scopes::assign_roles(graph);
    scopes::needs_execution(graph);
    scopes::check_scopes(graph, &mut steps.scopes);
    scopes::closure_scopes(graph, declared, &mut steps.scopes);
    scopes::check_closures(graph, declared, &mut steps.scopes);
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
        Dependencies::default(),
        location,
    ));
    node.exports.push(ExportRecord { key, reexport: false, location });
    let index = registry.nodes.len();
    registry.nodes.push(node);
    registry.by_identity.insert(identity, index);
    registry.imports.push(Vec::new());
    registry.post_order.insert(0, index);
}

/// Appends the registry's modules to `graph` in collection order, assigning ids and roles,
/// requalifying keyed exports, mounting controllers, and freezing metadata. Along the way it
/// reports import cycles (step 1) and, for step 2, the `try_value` failures (redacted), a
/// replacement's missing exports, a replacement whose original nothing imports, a second
/// replacement of one original, the test plan's overrides as it applies them, and qualified
/// contributions and single bindings under a role key. `load` numbers the modules of a lazy load.
///
/// A binding's role is decided here: `controller`, a contribution under a role key once every
/// handler is mounted (`scopes::mark_role_contributions`), or a provider. `scopes::assign_roles`
/// adds the bindings an `EnhancerSpec` names by type once the tables exist.
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

    // The registration walk applies the first replacement of an identity; a later one is
    // reported here rather than as unmatched.
    let replacements: &[Replacement] = plan.as_ref().map(|plan| plan.replacements.as_slice()).unwrap_or(&[]);
    for (position, replacement) in replacements.iter().enumerate() {
        if let Some(first) = replacements[..position].iter().find(|earlier| earlier.original == replacement.original) {
            steps.bindings.push(WiringError::DuplicateReplacement {
                original: ModuleName::of(&replacement.original, 0),
                first: first.location,
                second: replacement.location,
            });
        } else if !registry.replaced.iter().any(|replaced| replaced.original == replacement.original) {
            steps.bindings.push(WiringError::ReplacementUnmatched {
                original: ModuleName::of(&replacement.original, 0),
                at: replacement.location,
            });
        }
    }

    // Paired by order before any override runs: only a `try_value`, on `ModuleDef` or on
    // `Contribute`, writes a `Failed` record, and it records the failure in the same call. An
    // override may then replace a failed recipe, and `override_many` remove a failed
    // contribution; a failure whose record no longer fails is not reported.
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

    let replaced_collections: HashSet<Key> = collections.iter().map(|co| co.key).collect();
    for (index, position, failure) in failures {
        let Some(record) = registry.nodes[index].bindings.get(position) else { continue };
        if !matches!(record.recipe, Recipe::Failed) {
            continue;
        }
        // `apply_collections` below removes it.
        if record.kind == BindingKind::Collection && replaced_collections.contains(&record_key(record)) {
            continue;
        }
        steps.bindings.push(WiringError::ValueFailed {
            module: names[index].clone(),
            key: record_key(record).name(record.kind),
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
        transport_inputs: Vec::new(),
        meta: Vec::new(),
    };

    let mut contributions: HashMap<Key, Vec<BindingId>> = HashMap::new();
    let mut transports_seen: HashSet<TypeName> = graph.handlers.iter().map(|handler| handler.decl.transport).collect();
    let mut nodes: Vec<Option<ModuleNode>> = registry.nodes.into_iter().map(Some).collect();
    for (position, &index) in registry.post_order.iter().enumerate() {
        let Some(node) = nodes[index].take() else { continue };
        let id = ModuleId((first_module + position) as u32);
        let ModuleNode { identity, global, keyed, bindings, enhancers, exports, controllers, inputs, hooks, meta, .. } =
            node;

        let mut binding_ids = Vec::with_capacity(bindings.len());
        for (slot, record) in bindings.into_iter().enumerate() {
            let binding = BindingId(graph.bindings.len() as u32);
            if record.kind == BindingKind::Collection {
                contributions.entry(record_key(&record)).or_default().push(binding);
            }
            let role = if record.controller {
                Role::Controller
            } else if enhancers.contains(&slot) {
                Role::Enhancer
            } else {
                Role::Provider
            };
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
            let controller_key = graph.key_name(binding);
            let mut decls = Vec::new();
            (controller.mount)(&mut Mount::new(&mut decls));
            for decl in &decls {
                declare_transport_inputs(graph, decl, &mut transports_seen, &mut declared.transport_inputs);
            }
            graph.handlers.extend(
                decls
                    .into_iter()
                    .map(|decl| HandlerRecord::new(binding, controller_key, id, controller.prefix.clone(), decl)),
            );
        }

        for input in inputs {
            graph.inputs.entry(input.key).or_insert_with(|| InputDecl {
                key: input.key,
                seeder: input.seeder,
                origin: InputOrigin::Module {
                    module: names[index].clone(),
                    seeders: vec![input.seeder],
                    at: input.location,
                },
            });
            declared.inputs.push((id, input));
        }

        let mut frozen_meta = FrozenMeta::new();
        let mut entries: Vec<_> = meta.values.into_iter().collect();
        entries.sort_by_key(|(_, entry)| entry.name);
        for (ty, entry) in entries {
            let mut dependencies = Dependencies::default();
            (entry.dependencies)(&*entry.value, &mut dependencies);
            declared.meta.push((id, entry.name, dependencies));
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
    scopes::mark_role_contributions(graph, declared.first_binding, &mut steps.bindings);
    declared
}

/// Step 2, tests: each override replaces the recipe and dependencies of the bindings it matches,
/// which keep their origin module, scope, visibility, exports, hooks and readiness check. An
/// override of an `also_as` key splits that key off into a binding of its own in the same module,
/// since the override's value is not the type the original builds.
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
                    errors.push(WiringError::OverrideUnmatched {
                        key: ov.key.name(BindingKind::Single),
                        keyed: None,
                        at: ov.location,
                    });
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
                WiringError::OverrideUnmatched {
                    key: ov.key.name(BindingKind::Single),
                    keyed: keyed_binder(registry, names, &modules, ov.key),
                    at: ov.location,
                }
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
            errors.push(WiringError::OverrideUnmatched { key: ov.key.name(BindingKind::Single), keyed: None, at: ov.location });
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

/// A module among `modules` keyed by `key`'s qualifier that binds `key` unqualified. A keyed
/// module's own bindings carry no qualifier, so an override written `.qualified::<Q>()` misses
/// the binding `.in_module_keyed::<M, Q>()` reaches as written; the report names the module.
fn keyed_binder(registry: &Registry, names: &[ModuleName], modules: &[usize], key: Key) -> Option<ModuleName> {
    if key.is_unqualified() {
        return None;
    }
    let bare = unqualified(key);
    modules
        .iter()
        .copied()
        .find(|&module| {
            let node = &registry.nodes[module];
            node.keyed.is_some_and(|q| q.id == key.qualifier_id())
                && node.bindings.iter().any(|record| record.kind == BindingKind::Single && record.keys().any(|k| k == bare))
        })
        .map(|module| names[module].clone())
}

fn replace_recipe(node: &mut ModuleNode, position: usize, ov: &Override) {
    let Some(record) = node.bindings.get_mut(position) else { return };
    if record_key(record) == ov.key {
        record.recipe = ov.recipe.clone();
        record.dependencies = ov.dependencies.clone();
        return;
    }
    let qualifier = record.qualifier;
    let scope = record.scope;
    record.also.retain(|also| also.key.with_qualifier(qualifier.id, qualifier.name) != ov.key);
    let mut split = BindingRecord::new(
        unqualified(ov.key),
        ov.key.type_name().full(),
        BindingKind::Single,
        scope,
        ov.recipe.clone(),
        ov.dependencies.clone(),
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
                WiringError::OverrideUnmatched { key: co.key.name(BindingKind::Collection), keyed: None, at: co.location }
            });
            continue;
        }
        let Some(root) = registry.nodes.first_mut() else { continue };
        for item in co.items {
            // The item is already an `Arc` of the collection's type, so it needs no
            // `into_primary`.
            let mut record = BindingRecord::new(
                unqualified(co.key),
                co.key.type_name().full(),
                BindingKind::Collection,
                ScopeKind::Singleton,
                Recipe::Value(item),
                Dependencies::default(),
                co.location,
            );
            record.qualifier = qualifier_of(co.key);
            root.bindings.push(record);
        }
    }
}

/// Removes `node`'s contributions to `key`, keeping each controller and each enhancer mark
/// pointed at its own record.
fn remove_contributions(node: &mut ModuleNode, key: Key) -> usize {
    let mut removed_before = Vec::with_capacity(node.bindings.len());
    let mut gone = Vec::with_capacity(node.bindings.len());
    let mut kept = Vec::with_capacity(node.bindings.len());
    let mut removed = 0;
    for record in node.bindings.drain(..) {
        removed_before.push(removed);
        let remove = record.kind == BindingKind::Collection && record_key(&record) == key;
        gone.push(remove);
        if remove {
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
    node.enhancers.retain(|&position| !gone.get(position).copied().unwrap_or(false));
    for position in &mut node.enhancers {
        if let Some(&shift) = removed_before.get(*position) {
            *position -= shift;
        }
    }
    removed
}

/// `key` under `()`: a record's `primary` is unqualified, its qualifier held apart, and an
/// override's key may carry one since `.qualified::<Q>()`.
fn unqualified(key: Key) -> Key {
    let none = Qualifier::none();
    key.with_qualifier(none.id, none.name)
}

fn qualifier_of(key: Key) -> Qualifier {
    let none = Qualifier::none();
    Qualifier { id: key.qualifier_id(), name: key.qualifier_name().unwrap_or(none.name) }
}

/// `Transport::inputs` of `decl`'s transport, the first time a handler of that transport mounts
/// (transports DESIGN §2.10, X4). Each key is recorded with the transport as its seeder and its
/// origin, in `graph` and in `declared` for step 2. A module's own declaration of the same key
/// with the same seeder is the same declaration; one with another seeder, another transport's
/// declaration of the key, or a single binding under it is reported by step 2.
fn declare_transport_inputs(graph: &mut Graph, decl: &HandlerDecl, seen: &mut HashSet<TypeName>, declared: &mut Vec<InputDecl>) {
    if !seen.insert(decl.transport) {
        return;
    }
    let mut inputs = Inputs::new();
    (decl.inputs)(&mut inputs);
    for input in inputs.keys {
        let declaration = InputDecl {
            key: input.key,
            seeder: decl.transport,
            origin: InputOrigin::Transport { name: decl.transport, at: input.location },
        };
        graph.inputs.entry(input.key).or_insert_with(|| declaration.clone());
        declared.push(declaration);
    }
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
            // The module binds no such key, so a binding its table holds came from an import or a
            // global.
            let imported = matches!(graph.lookup(*module, export.key), Some(Visible::Binding(_) | Visible::Ambiguous(_)));
            errors.push(WiringError::ExportNotBound {
                module: graph.module_name(*module),
                key,
                imported,
                near: visibility::own_near_spelling(graph, *module, export.key).map(|near| near.name(BindingKind::Single)),
                at: export.location,
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
/// readiness checks on one binding, an execution input declared by two modules or also bound,
/// and an input a transport declares that a module declares with another seeder, another
/// transport also declares, or a module binds.
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
    // two kinds. Reported once per key, where either side is new to this wiring. Under a role
    // key, freezing reported the single binding as `SingleRoleBinding`.
    let roles = scopes::role_types(graph, &graph.bindings);
    let mut mixed: HashSet<Key> = HashSet::new();
    for binding in &graph.bindings {
        if binding.record.kind != BindingKind::Single {
            continue;
        }
        for key in binding.record.keys() {
            if roles.contains(&key.type_id()) {
                continue;
            }
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

    // A transport's declaration and a module's of the same key and seeder are one declaration;
    // with another seeder, or beside another transport's or a single binding, the input has two
    // sources and a handler of one of the transports would read it unseeded.
    for (index, input) in declared.transport_inputs.iter().enumerate() {
        let key = || input.key.name(BindingKind::Single);
        for (module, other) in declared.inputs.iter().filter(|(_, other)| other.key == input.key && other.seeder != input.seeder) {
            let second = InputOrigin::Module {
                module: graph.module_name(*module),
                seeders: vec![other.seeder],
                at: other.location,
            };
            errors.push(WiringError::InputConflict { key: key(), first: input.origin.clone(), second });
        }
        for other in declared.transport_inputs[..index].iter().filter(|other| other.key == input.key && other.seeder != input.seeder) {
            errors.push(WiringError::InputConflict { key: key(), first: other.origin.clone(), second: input.origin.clone() });
        }
        // A binding under the key is reported once: by the module loop above when a module
        // declares the input too, otherwise against the key's first transport declaration.
        let reported = declared.inputs.iter().any(|(_, other)| other.key == input.key)
            || declared.transport_inputs[..index].iter().any(|other| other.key == input.key);
        if reported {
            continue;
        }
        for binding in &graph.bindings {
            if binding.record.kind == BindingKind::Single && binding.record.keys().any(|bound| bound == input.key) {
                let second = InputOrigin::Binding { module: graph.module_name(binding.origin), at: binding.record.location };
                errors.push(WiringError::InputConflict { key: key(), first: input.origin.clone(), second });
            }
        }
    }
}

/// Step 6: a `Timer` wherever a wait is written: a readiness `.timeout`, `.attempt_timeout` or
/// `.backoff`, a hook's or constructor's `After(..)`, or a builder knob. A lazy load checks what
/// it brought; the knobs were checked when the app wired.
///
/// The record holds a backoff as a `Duration` that is zero unless written, so `.backoff(ZERO)`
/// reads as unwritten; it waits for nothing, and nothing needs a `Timer` for it.
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
            if !ready.backoff.is_zero() {
                errors.push(WiringError::BackoffWithoutTimer {
                    binding: name.clone(),
                    at: ready.backoff_location.unwrap_or(ready.location),
                });
            }
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

/// Whether anything in `base` reads `key` as a collection: a binding's dependency, a readiness
/// check or hook closure, a module hook, a handler's parameter, an enhancer closure, or a
/// transport's role collection.
fn reads_collection(base: &Graph, key: Key) -> bool {
    let in_dependencies = |dependencies: &Dependencies| {
        dependencies.list.iter().flat_map(|d| &d.requirement.reads).any(|read| matches!(&read.kind, ReadKind::Collection(k) if *k == key))
    };
    base.bindings.iter().any(|binding| {
        binding.edges.iter().any(|edge| matches!(&edge.target, EdgeTarget::Collection(k) if *k == key))
            || binding.record.ready.as_ref().is_some_and(|ready| in_dependencies(&ready.dependencies))
            || binding.record.hooks.iter().any(|hook| in_dependencies(&hook.dependencies))
    }) || base.modules.iter().any(|module| module.hooks.iter().any(|hook| in_dependencies(&hook.dependencies)))
        || base.handlers.iter().any(|handler| {
            handler.decl.role_keys.contains(&key)
                || in_dependencies(&handler.decl.dependencies)
                || handler.decl.enhancer_deps.iter().any(|dep| match dep {
                    EnhancerDep::Closure(closure) => in_dependencies(&closure.dependencies),
                    EnhancerDep::Type(_) => false,
                })
        })
}
