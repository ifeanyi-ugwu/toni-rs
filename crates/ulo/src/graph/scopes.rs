//! Step 5: the needs-execution pass, scope violations, hooks on bindings inferred per-execution,
//! and the per-handler input check (§6.2, §6.4).
//!
//! A binding needs an execution if any of its sites is `Ext`, `ExecutionRef`, an execution
//! input, or a dependency that itself needs one. Transient and `Auto` bindings pass the need
//! upward. Then:
//!
//! | Binding | Needs an execution | Result |
//! |---|---|---|
//! | Explicit singleton | yes | refused, with the full path |
//! | Auto provider | yes | refused, hint "declare it `#[injectable(execution)]`" |
//! | Auto controller or enhancer | yes | per-execution, built per call |
//! | Auto with hooks, inferred per-execution | — | refused |
//! | Transient | yes | allowed; every consumer must be able to run in an execution |
//!
//! A binding refused this way stays a singleton for its readers, so one violation is reported
//! once, at the binding that introduces it, and not again at everything above it.

use std::any::type_name;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::binding::Recipe;
use crate::error::wiring::WiringError;
use crate::graph::{BindingId, EdgeTarget, Effective, FrozenBinding, Graph, Role, Visible, record_key, site_text};
use crate::key::{BindingKind, Key};
use crate::scope::ScopeKind;
use crate::site::ReadKind;
use crate::transport::controller::{EnhancerDep, HandlerRecord};
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, Transport};

/// Marks enhancer roles: bindings named by type in an `EnhancerSpec`, resolved against the
/// controller module's visibility, and contributions under a role key. Runs before
/// `needs_execution`.
///
/// A role key is `AnyGuard<T>`, `AnyInterceptor<T>` or `AnyErrorHandler<T>` for any transport
/// `T`, whether or not `T` has a handler: the keys a mounted handler names, and any key of the
/// same three families. A global enhancer for a transport the app mounts nothing on is still an
/// enhancer, never resolved, rather than a provider refused for reading execution data.
pub(crate) fn assign_roles(graph: &mut Graph) {
    let mounted: HashSet<Key> = graph.handlers.iter().flat_map(|handler| handler.decl.role_keys).collect();
    let families = role_families();

    let mut named: Vec<BindingId> = Vec::new();
    for handler in &graph.handlers {
        for dep in &handler.decl.enhancer_deps {
            if let EnhancerDep::Type(key) = dep {
                if let Some(Visible::Binding(id)) = graph.lookup(handler.module, *key) {
                    named.push(through_aliases(graph, *id));
                }
            }
        }
    }

    for binding in &mut graph.bindings {
        let record = &binding.record;
        binding.role = if record.controller {
            Role::Controller
        } else if record.kind == BindingKind::Collection && is_role_key(&mounted, &families, record_key(record)) {
            Role::Enhancer
        } else {
            Role::Provider
        };
    }
    for id in named {
        let binding = &mut graph.bindings[id.0 as usize];
        if binding.role == Role::Provider {
            binding.role = Role::Enhancer;
        }
    }
}

/// Sets `needs_execution` and `effective` on every binding, after the cycle check. A cycle the
/// check reported does not stop the pass: each binding changes at most once, so it ends.
pub(crate) fn needs_execution(graph: &mut Graph) {
    let count = graph.bindings.len();
    for binding in &mut graph.bindings {
        binding.effective = match binding.record.scope {
            ScopeKind::Singleton | ScopeKind::Auto => Effective::Singleton,
            ScopeKind::PerExecution => Effective::PerExecution,
            ScopeKind::Transient => Effective::Transient,
        };
        binding.needs_execution = binding.record.scope == ScopeKind::PerExecution
            || binding.edges.iter().any(|edge| matches!(edge.target, EdgeTarget::Execution | EdgeTarget::Input(_)));
        if binding.needs_execution {
            promote(binding);
        }
    }

    let mut readers: Vec<Vec<BindingId>> = vec![Vec::new(); count];
    for index in 0..count {
        let reader = BindingId(index as u32);
        for dep in graph.construction_deps(reader) {
            readers[dep.0 as usize].push(reader);
        }
    }

    let mut queue: VecDeque<BindingId> =
        (0..count).map(|index| BindingId(index as u32)).filter(|&id| graph.passes_execution(id)).collect();
    while let Some(dep) = queue.pop_front() {
        for &reader in &readers[dep.0 as usize] {
            {
                let binding = &mut graph.bindings[reader.0 as usize];
                if binding.needs_execution {
                    continue;
                }
                binding.needs_execution = true;
                promote(binding);
            }
            if graph.passes_execution(reader) {
                queue.push_back(reader);
            }
        }
    }
}

/// An `Auto` controller or enhancer that needs an execution is built per call (§6.2). An `Auto`
/// provider stays a singleton and is refused by `check_scopes`.
fn promote(binding: &mut FrozenBinding) {
    if binding.record.scope == ScopeKind::Auto && binding.role != Role::Provider {
        binding.effective = Effective::PerExecution;
    }
}

/// Scope violations with the path that introduces the execution dependency, and hooks on
/// bindings inferred per-execution. A readiness check counts as a hook here: `connect` runs it,
/// and a binding built per call never reaches `connect`.
pub(crate) fn check_scopes(graph: &Graph, errors: &mut Vec<WiringError>) {
    for binding in &graph.bindings {
        let record = &binding.record;
        if matches!(record.recipe, Recipe::Alias { .. } | Recipe::Failed) {
            continue;
        }
        let refused = binding.needs_execution
            && match record.scope {
                ScopeKind::Singleton => true,
                ScopeKind::Auto => binding.role == Role::Provider,
                ScopeKind::PerExecution | ScopeKind::Transient => false,
            };
        if refused {
            errors.push(WiringError::ScopeViolation {
                binding: graph.key_name(binding.id),
                declared: record.scope,
                module: graph.module_name(binding.origin),
                path: printed_path(graph, binding.id),
            });
        } else if record.scope == ScopeKind::Auto
            && binding.effective == Effective::PerExecution
            && (!record.hooks.is_empty() || record.ready.is_some())
        {
            errors.push(WiringError::HooksOnPerExecution {
                binding: graph.key_name(binding.id),
                module: graph.module_name(binding.origin),
            });
        }
    }
}

/// For each handler, every non-optional input on its reachable execution-scoped bindings that
/// the handler's transport does not seed, with the path from the handler to the service that
/// reads it, the handler's transport and the input's seeder.
///
/// What a handler reaches: its controller, the global enhancers under its transport's role
/// keys, the bindings its by-type enhancers name, and what its enhancer closures read. The walk
/// enters execution-scoped and transient bindings only, since a singleton is built at `connect`
/// with no execution, and one reading an input has already failed the scope check. Each input is
/// reported once per handler and reading binding.
pub(crate) fn check_inputs(graph: &Graph, errors: &mut Vec<WiringError>) {
    for handler in &graph.handlers {
        InputWalk::new(graph, handler).run(errors);
    }
}

struct InputWalk<'g> {
    graph: &'g Graph,
    handler: &'g HandlerRecord,
    /// The handler as the first step of a path: `UsersController::get_rpc (Rpc)`.
    head: String,
    reported: HashSet<(Key, Option<BindingId>)>,
}

impl<'g> InputWalk<'g> {
    fn new(graph: &'g Graph, handler: &'g HandlerRecord) -> Self {
        let head = format!("{} ({})", graph.handler_name(handler), handler.decl.transport_name);
        InputWalk { graph, handler, head, reported: HashSet::new() }
    }

    fn run(mut self, errors: &mut Vec<WiringError>) {
        let graph = self.graph;
        let handler = self.handler;
        let mut roots = vec![handler.controller];
        for key in handler.decl.role_keys {
            roots.extend(graph.collection(key).iter().copied());
        }
        for dep in &handler.decl.enhancer_deps {
            match dep {
                EnhancerDep::Type(key) => {
                    if let Some(Visible::Binding(id)) = graph.lookup(handler.module, *key) {
                        roots.push(*id);
                    }
                }
                EnhancerDep::Closure(sites) => {
                    for site in &sites.list {
                        for read in &site.desc.reads {
                            match &read.kind {
                                ReadKind::Single(key) => match graph.lookup(handler.module, *key) {
                                    Some(Visible::Input(input)) if !read.optional => {
                                        let path = vec![self.head.clone(), "enhancer closure".to_owned(), site_text(site)];
                                        self.report(*input, None, path, errors);
                                    }
                                    Some(Visible::Binding(id)) => roots.push(*id),
                                    _ => {}
                                },
                                ReadKind::Collection(key) => roots.extend(graph.collection(*key).iter().copied()),
                                ReadKind::Extension(_) | ReadKind::Execution | ReadKind::Module => {}
                            }
                        }
                    }
                }
            }
        }

        let mut parent: HashMap<BindingId, Option<BindingId>> = HashMap::new();
        let mut queue = VecDeque::new();
        for root in roots {
            if in_execution(graph, root) && !parent.contains_key(&root) {
                parent.insert(root, None);
                queue.push_back(root);
            }
        }
        while let Some(node) = queue.pop_front() {
            for edge in &graph.binding(node).edges {
                if let EdgeTarget::Input(input) = &edge.target {
                    if !edge.optional {
                        let mut path = vec![self.head.clone()];
                        path.extend(chain(&parent, node).into_iter().map(|step| graph.scoped_label(step)));
                        path.push(graph.site_step(node, edge.site));
                        self.report(*input, Some(node), path, errors);
                    }
                }
            }
            for dep in graph.construction_deps(node) {
                if in_execution(graph, dep) && !parent.contains_key(&dep) {
                    parent.insert(dep, Some(node));
                    queue.push_back(dep);
                }
            }
        }
    }

    fn report(&mut self, input: Key, reader: Option<BindingId>, path: Vec<String>, errors: &mut Vec<WiringError>) {
        let Some(decl) = self.graph.inputs.get(&input) else { return };
        if decl.seeder == self.handler.decl.transport || !self.reported.insert((input, reader)) {
            return;
        }
        errors.push(WiringError::InputNotSeeded {
            handler: self.graph.handler_name(self.handler),
            transport: self.handler.decl.transport_name,
            input: input.name(BindingKind::Single),
            seeder: decl.seeder_name,
            path,
        });
    }
}

fn in_execution(graph: &Graph, id: BindingId) -> bool {
    matches!(graph.binding(id).effective, Effective::PerExecution | Effective::Transient)
}

/// The walk's path from its root to `node`, root first.
fn chain(parent: &HashMap<BindingId, Option<BindingId>>, node: BindingId) -> Vec<BindingId> {
    let mut steps = vec![node];
    let mut current = node;
    while let Some(&Some(previous)) = parent.get(&current) {
        if steps.contains(&previous) {
            break;
        }
        steps.push(previous);
        current = previous;
    }
    steps.reverse();
    steps
}

/// The dependency path from `from` to the first binding reading execution data, for a report.
/// The walk follows only dependencies that pass the need upward, and stops at a binding that
/// reads execution data itself, or at an execution-scoped one, whose own sites need not read any.
pub(crate) fn execution_path(graph: &Graph, from: BindingId) -> Vec<BindingId> {
    let reached = |id: BindingId| {
        graph.direct_execution_edge(id).is_some() || (id != from && graph.binding(id).effective == Effective::PerExecution)
    };
    if reached(from) {
        return vec![from];
    }
    let mut parent: HashMap<BindingId, BindingId> = HashMap::new();
    let mut queue = VecDeque::from([from]);
    while let Some(node) = queue.pop_front() {
        for dep in graph.construction_deps(node) {
            if dep == from || parent.contains_key(&dep) || !graph.passes_execution(dep) {
                continue;
            }
            parent.insert(dep, node);
            if reached(dep) {
                let mut path = vec![dep];
                let mut current = dep;
                while let Some(&previous) = parent.get(&current) {
                    path.push(previous);
                    if previous == from {
                        break;
                    }
                    current = previous;
                }
                path.reverse();
                return path;
            }
            queue.push_back(dep);
        }
    }
    vec![from]
}

/// `execution_path` as a report prints it: the binding, each step with its scope, then the site
/// that reads execution data, as in `ReportService → AuditContext (execution) → Ext<CurrentUser>`.
fn printed_path(graph: &Graph, from: BindingId) -> Vec<String> {
    let ids = execution_path(graph, from);
    let mut steps: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(position, &id)| if position == 0 { graph.label(id) } else { graph.scoped_label(id) })
        .collect();
    if let Some(&last) = ids.last() {
        if let Some(edge) = graph.direct_execution_edge(last) {
            steps.push(graph.site_step(last, edge.site));
        }
    }
    steps
}

/// The binding an alias chain ends at, bounded by the binding count so a looping chain, which
/// step 4 reports, ends too.
fn through_aliases(graph: &Graph, mut id: BindingId) -> BindingId {
    for _ in 0..graph.bindings.len() {
        match graph.alias_target(id) {
            Some(target) => id = target,
            None => break,
        }
    }
    id
}

/// A transport no handler mounts on, whose role keys' type names give the three families'
/// prefixes.
struct NoTransport;

impl Transport for NoTransport {
    type Cx = ();
    type Reply = ();
}

/// `dyn ulo::transport::ErasedGuard<` and its two siblings, as `type_name` writes them in this
/// build, so a key's name is compared with a prefix taken the same way.
fn role_families() -> [&'static str; 3] {
    [
        family(type_name::<AnyGuard<NoTransport>>()),
        family(type_name::<AnyInterceptor<NoTransport>>()),
        family(type_name::<AnyErrorHandler<NoTransport>>()),
    ]
}

fn family(name: &'static str) -> &'static str {
    match name.find('<') {
        Some(open) => &name[..=open],
        None => name,
    }
}

fn is_role_key(mounted: &HashSet<Key>, families: &[&'static str; 3], key: Key) -> bool {
    mounted.contains(&key) || families.iter().any(|family| key.type_name().starts_with(*family))
}
