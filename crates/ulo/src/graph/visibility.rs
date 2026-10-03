//! Step 3: the visibility tables and every injection point resolved against its module's table
//! (§8.2, §10.1 step 3).

use std::collections::{HashMap, HashSet};

use crate::dependency::{Dependencies, ReadKind};
use crate::error::wiring::WiringError;
use crate::graph::wire::Declared;
use crate::graph::{
    BindingId, Edge, EdgeTarget, Graph, ModuleId, Visible, VisibilityTable, boundary_key, dependency_label, record_key,
};
use crate::key::{BindingKind, Key, short_type_name};
use crate::transport::controller::EnhancerDep;

/// Fills `graph.visibility`: each module's own bindings, its direct imports' exports and the
/// globals' exports, a key with two sources recorded as `Visible::Ambiguous`. Execution inputs
/// are app-wide and in every table, below any binding under the same key.
///
/// Only the modules this wiring added get tables; a lazy load's base modules keep theirs, and
/// their resolved exports are read from `graph.exported`.
pub(crate) fn build_tables(graph: &mut Graph, declared: &Declared) {
    let count = graph.modules.len();
    graph.visibility.resize_with(count, VisibilityTable::default);
    graph.exported.resize_with(count, Vec::new);

    for (module, export) in declared.exports.iter().filter(|(_, export)| !export.reexport) {
        if let Some(id) = own_binding(graph, *module, export.key) {
            let key = boundary_key(graph.module(*module).keyed, export.key);
            graph.exported[module.0 as usize].push((key, id));
        }
    }

    // A re-export reads its module's view, which holds other modules' exports, re-exports
    // among them; each round resolves what the last round made visible. One that never resolves
    // is reported by step 1 as not visible or ambiguous.
    let mut pending: Vec<&(ModuleId, crate::module::def::ExportRecord)> =
        declared.exports.iter().filter(|(_, export)| export.reexport).collect();
    loop {
        let view: &Graph = graph;
        let mut resolved = Vec::new();
        pending.retain(|entry| {
            let (module, export) = &**entry;
            match sources(view, *module, export.key).as_slice() {
                [(_, id)] => {
                    resolved.push((*module, boundary_key(view.module(*module).keyed, export.key), *id));
                    false
                }
                _ => true,
            }
        });
        if resolved.is_empty() {
            break;
        }
        for (module, key, id) in resolved {
            graph.exported[module.0 as usize].push((key, id));
        }
    }

    for index in declared.first_module..count {
        let table = table_for(graph, ModuleId(index as u32));
        graph.visibility[index] = table;
    }
}

/// The first single binding of `module` itself answering to `key`. A second one is a duplicate
/// that step 2 reports.
pub(crate) fn own_binding(graph: &Graph, module: ModuleId, key: Key) -> Option<BindingId> {
    graph.module(module).bindings.iter().copied().find(|&id| {
        let record = &graph.binding(id).record;
        record.kind == BindingKind::Single && record.keys().any(|own| own == key)
    })
}

/// Every source `module` sees for `key`, one per binding: its own binding first, then its
/// imports' and the globals' exports. One binding reached through two routes is one source.
pub(crate) fn sources(graph: &Graph, module: ModuleId, key: Key) -> Vec<(ModuleId, BindingId)> {
    let mut found: Vec<(ModuleId, BindingId)> = own_binding(graph, module, key).map(|id| (module, id)).into_iter().collect();
    for exporter in exporters(graph, module) {
        for &(exported, id) in &graph.exported[exporter.0 as usize] {
            if exported == key && !found.iter().any(|&(_, seen)| seen == id) {
                found.push((exporter, id));
            }
        }
    }
    found
}

/// The modules whose exports `module` sees: its direct imports, then every global module.
fn exporters(graph: &Graph, module: ModuleId) -> impl Iterator<Item = ModuleId> + '_ {
    let imports = graph.module(module).imports.iter().copied();
    let globals = graph.modules.iter().filter(|m| m.global).map(|m| m.id);
    imports.chain(globals)
}

fn table_for(graph: &Graph, module: ModuleId) -> VisibilityTable {
    let mut sources: HashMap<Key, Vec<(ModuleId, BindingId)>> = HashMap::new();
    for &id in &graph.module(module).bindings {
        let record = &graph.binding(id).record;
        if record.kind != BindingKind::Single {
            continue;
        }
        for key in record.keys() {
            sources.entry(key).or_insert_with(|| vec![(module, id)]);
        }
    }
    for exporter in exporters(graph, module) {
        for &(key, id) in &graph.exported[exporter.0 as usize] {
            let list = sources.entry(key).or_default();
            if !list.iter().any(|&(_, seen)| seen == id) {
                list.push((exporter, id));
            }
        }
    }

    let mut entries: HashMap<Key, Visible> = sources
        .into_iter()
        .map(|(key, list)| {
            let visible = if list.len() == 1 { Visible::Binding(list[0].1) } else { Visible::Ambiguous(list) };
            (key, visible)
        })
        .collect();
    for &key in graph.inputs.keys() {
        entries.entry(key).or_insert(Visible::Input(key));
    }
    VisibilityTable { entries }
}

/// Resolves every injection point of every binding, hook closure, readiness check, enhancer
/// declaration and metadata value into `Edge`s, and reports missing keys (with the injection
/// point, the key and the module) and ambiguous keys (naming every source module). A missing
/// key whose name equals a bound key's name up to a trailing
/// `+ core::marker::Send + core::marker::Sync` names both spellings.
///
/// Only a binding's own dependencies become edges. Readiness checks, hooks, enhancer closures
/// and metadata are checked for missing and ambiguous keys alone; the passes that need what they
/// read resolve it again from the tables.
///
/// The root's table is then swept: an ambiguous key there is reported even when nothing reads
/// it, because a lookup naming no module uses the root's visibility and has no variant for an
/// ambiguity (§8.2).
pub(crate) fn resolve_dependencies(graph: &mut Graph, declared: &Declared, errors: &mut Vec<WiringError>) {
    let mut reported: HashSet<(ModuleId, Key)> = HashSet::new();

    for index in declared.first_binding..graph.bindings.len() {
        let edges = binding_edges(graph, BindingId(index as u32), errors, &mut reported);
        graph.bindings[index].edges = edges;
    }

    let graph = &*graph;
    for binding in graph.bindings.iter().skip(declared.first_binding) {
        if let Some(ready) = &binding.record.ready {
            let owner = format!("readiness check of {}", graph.label(binding.id));
            check_dependencies(graph, binding.origin, &ready.dependencies, &owner, errors, &mut reported);
        }
        for hook in &binding.record.hooks {
            let owner = format!("{} hook of {}", hook.kind, graph.label(binding.id));
            check_dependencies(graph, binding.origin, &hook.dependencies, &owner, errors, &mut reported);
        }
    }
    for module in graph.modules.iter().skip(declared.first_module) {
        for hook in &module.hooks {
            let owner = format!("{} hook of {}", hook.kind, module.name);
            check_dependencies(graph, module.id, &hook.dependencies, &owner, errors, &mut reported);
        }
    }
    for (module, name, dependencies) in &declared.meta {
        let owner = format!("metadata `{}` of {}", short_type_name(*name), graph.module_name(*module));
        check_dependencies(graph, *module, dependencies, &owner, errors, &mut reported);
    }
    for handler in graph.handlers.iter().skip(declared.first_handler) {
        let name = graph.handler_name(handler);
        // The pipeline finds the controller by its key in its own module, which an import
        // exporting the same type would make ambiguous; nothing else reads that key.
        let controller = record_key(&graph.binding(handler.controller).record);
        if let Some(Visible::Ambiguous(list)) = graph.lookup(handler.module, controller) {
            if reported.insert((handler.module, controller)) {
                errors.push(WiringError::Ambiguous {
                    key: controller.name(BindingKind::Single),
                    consumer: format!("{name} (its controller)"),
                    module: graph.module_name(handler.module),
                    sources: ambiguous_sources(graph, list),
                });
            }
        }
        for dep in &handler.decl.enhancer_deps {
            match dep {
                EnhancerDep::Type(key) => {
                    let consumer = || format!("{name} (enhancer `{}`)", short_type_name(key.type_name()));
                    check_key(graph, handler.module, *key, false, consumer, errors, &mut reported);
                }
                EnhancerDep::Closure(closure) => {
                    let owner = format!("{name} (enhancer closure)");
                    check_dependencies(graph, handler.module, &closure.dependencies, &owner, errors, &mut reported);
                }
            }
        }
    }

    if declared.first_module == 0 {
        let root = graph.root;
        let mut unread: Vec<(&Key, &Vec<(ModuleId, BindingId)>)> = graph.visibility[root.0 as usize]
            .entries
            .iter()
            .filter_map(|(key, visible)| match visible {
                Visible::Ambiguous(list) if !reported.contains(&(root, *key)) => Some((key, list)),
                _ => None,
            })
            .collect();
        unread.sort_by_key(|(key, _)| (key.type_name(), key.qualifier_name()));
        for (key, list) in unread {
            errors.push(WiringError::Ambiguous {
                key: key.name(BindingKind::Single),
                consumer: "a lookup that names no module".to_owned(),
                module: graph.module_name(root),
                sources: ambiguous_sources(graph, list),
            });
        }
    }
}

fn binding_edges(
    graph: &Graph,
    id: BindingId,
    errors: &mut Vec<WiringError>,
    reported: &mut HashSet<(ModuleId, Key)>,
) -> Vec<Edge> {
    let binding = graph.binding(id);
    let mut edges = Vec::new();
    for (index, dependency) in binding.record.dependencies.list.iter().enumerate() {
        for read in &dependency.requirement.reads {
            let target = match &read.kind {
                ReadKind::Single(key) => {
                    let consumer = || graph.consumer(id, index);
                    check_key(graph, binding.origin, *key, read.optional, consumer, errors, reported)
                }
                ReadKind::Collection(key) => Some(EdgeTarget::Collection(*key)),
                ReadKind::Extension(_) | ReadKind::Execution => Some(EdgeTarget::Execution),
                ReadKind::Module => Some(EdgeTarget::Module),
            };
            if let Some(target) = target {
                edges.push(Edge { target, dependency: index, optional: read.optional });
            }
        }
    }
    edges
}

/// Every single-key read of `dependencies` checked against `module`'s table, each named
/// ``{owner} (param #1)``.
fn check_dependencies(
    graph: &Graph,
    module: ModuleId,
    dependencies: &Dependencies,
    owner: &str,
    errors: &mut Vec<WiringError>,
    reported: &mut HashSet<(ModuleId, Key)>,
) {
    for dependency in &dependencies.list {
        for read in &dependency.requirement.reads {
            if let ReadKind::Single(key) = &read.kind {
                let consumer = || format!("{owner} ({})", dependency_label(dependency.label));
                check_key(graph, module, *key, read.optional, consumer, errors, reported);
            }
        }
    }
}

/// What `key` resolves to in `module`, or the error a read of it is reported with. A
/// missing optional read is no error and no edge.
fn check_key(
    graph: &Graph,
    module: ModuleId,
    key: Key,
    optional: bool,
    consumer: impl FnOnce() -> String,
    errors: &mut Vec<WiringError>,
    reported: &mut HashSet<(ModuleId, Key)>,
) -> Option<EdgeTarget> {
    match graph.lookup(module, key) {
        Some(Visible::Binding(id)) => Some(EdgeTarget::Binding(*id)),
        Some(Visible::Input(input)) => Some(EdgeTarget::Input(*input)),
        Some(Visible::Ambiguous(list)) => {
            reported.insert((module, key));
            errors.push(WiringError::Ambiguous {
                key: key.name(BindingKind::Single),
                consumer: consumer(),
                module: graph.module_name(module),
                sources: ambiguous_sources(graph, list),
            });
            None
        }
        None => {
            if !optional {
                errors.push(WiringError::Missing {
                    key: key.name(BindingKind::Single),
                    consumer: consumer(),
                    module: graph.module_name(module),
                    near: near_spelling(graph, module, key)
                        .map(|(near, exporter)| (near.name(BindingKind::Single), graph.module_name(exporter))),
                });
            }
            None
        }
    }
}

fn ambiguous_sources(
    graph: &Graph,
    list: &[(ModuleId, BindingId)],
) -> Vec<(crate::module::ModuleName, &'static std::panic::Location<'static>)> {
    list.iter().map(|&(module, id)| (graph.module_name(module), graph.binding(id).record.location)).collect()
}

/// The bound key spelled like `missing`, for the hint on a missing dependency, with the module
/// providing it: one `module` sees first, then any binding in the graph. See `spelled_alike`.
pub(crate) fn near_spelling(graph: &Graph, module: ModuleId, missing: Key) -> Option<(Key, ModuleId)> {
    let near = |key: &Key| spelled_alike(*key, missing);

    let mut visible: Vec<(Key, ModuleId)> = graph.visibility[module.0 as usize]
        .entries
        .iter()
        .filter(|(key, _)| near(*key))
        .filter_map(|(key, visible)| match visible {
            Visible::Binding(id) => Some((*key, graph.binding(*id).origin)),
            Visible::Ambiguous(list) => list.first().map(|&(source, _)| (*key, source)),
            Visible::Input(_) => None,
        })
        .collect();
    visible.sort_by_key(|(key, _)| key.type_name());
    if let Some(found) = visible.into_iter().next() {
        return Some(found);
    }
    graph
        .bindings
        .iter()
        .find_map(|binding| binding.record.keys().find(|key| near(key)).map(|key| (key, binding.origin)))
}

/// A key `module` binds itself spelled like `exported`, for the hint on an export of a key the
/// module does not bind.
pub(crate) fn own_near_spelling(graph: &Graph, module: ModuleId, exported: Key) -> Option<Key> {
    graph.module(module).bindings.iter().find_map(|&id| {
        let record = &graph.binding(id).record;
        if record.kind != BindingKind::Single {
            return None;
        }
        record.keys().find(|key| spelled_alike(*key, exported))
    })
}

/// Two different keys under one qualifier that a report cannot tell apart at a glance: equal up
/// to trailing `Send` and `Sync` bounds, which are distinct `TypeId`s, or equal in their last path
/// segments, `a::Config` beside `b::Config`.
fn spelled_alike(key: Key, other: Key) -> bool {
    key.qualifier_id() == other.qualifier_id()
        && key.type_name() != other.type_name()
        && short_type_name(strip_auto_traits(key.type_name())) == short_type_name(strip_auto_traits(other.type_name()))
}

/// `name` without trailing auto-trait bounds, in either order and either spelling.
fn strip_auto_traits(name: &str) -> &str {
    const SUFFIXES: [&str; 4] = [" + core::marker::Send", " + core::marker::Sync", " + Send", " + Sync"];
    let mut name = name;
    while let Some(rest) = SUFFIXES.iter().find_map(|suffix| name.strip_suffix(*suffix)) {
        name = rest;
    }
    name
}
