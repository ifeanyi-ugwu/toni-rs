//! Step 5: the needs-execution pass, scope violations, hooks on bindings built per execution,
//! the scope of each enhancer declared by closure, closures that read execution data where none
//! exists, and the per-handler input check (§6.2, §6.4).
//!
//! A binding needs an execution if any of its injection points is `Ext`, `ExecutionRef`, an
//! execution input, or a dependency that itself needs one. Transient and `Auto` bindings pass the
//! need upward. Then:
//!
//! | Binding | Needs an execution | Result |
//! |---|---|---|
//! | Explicit singleton | yes | refused, with the full path |
//! | Auto provider | yes | refused, hint "declare it `#[injectable(execution)]`" |
//! | Auto controller or enhancer | yes | per-execution, built per call |
//! | Auto with hooks, inferred per-execution | — | refused |
//! | Transient | yes | allowed; every consumer must be able to run in an execution |
//!
//! A binding declared by closure follows the same table: `Contribute::with` registers `Auto`, and
//! the explicit forms register their scope. Only the hint differs, naming the closure's explicit
//! form instead of the attribute. An enhancer declared by closure on a handler is no binding, and
//! `closure_scopes` applies the same rule to it.
//!
//! A binding refused this way stays a singleton for its readers, so one violation is reported
//! once, at the binding that introduces it, and not again at everything above it.

use std::any::TypeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::Location;

use crate::binding::Recipe;
use crate::dependency::{Dependencies, DependencyRecord, ReadKind};
use crate::error::wiring::WiringError;
use crate::graph::wire::Declared;
use crate::graph::{
    BindingId, EdgeTarget, Effective, FrozenBinding, Graph, ModuleId, Role, Visible, dependency_text, record_key,
};
use crate::key::{BindingKind, Key};
use crate::scope::ScopeKind;
use crate::transport::controller::{EnhancerDep, HandlerRecord};
use crate::type_name::short_type_name;

/// Gives the enhancer role to every contribution under a role key, whichever builder registered
/// it, and reports what a transport cannot read under one (step 2): a qualified contribution, as
/// `QualifiedRoleContribution`, and a single binding through any of its keys, as
/// `SingleRoleBinding`. The role keys are those `role_types` collects.
///
/// Keys compare by `TypeId` alone: an alias of a role key, or a `macro_rules` wrapper around one,
/// is the same key, and no type name is read. A qualified contribution still takes the role, so
/// the scope pass does not refuse it a second time as a provider. Runs at freeze, once every
/// handler is mounted; a lazy load re-runs it over the base bindings, whose roles only ever move
/// from provider to enhancer here.
///
/// A lazy load reports a binding it brought, and a base binding only under a role key the load
/// marked: the base wiring checked the others.
pub(crate) fn mark_role_contributions(graph: &mut Graph, first_binding: usize, errors: &mut Vec<WiringError>) {
    let roles = role_types(graph, &graph.bindings);
    let checked = role_types(graph, &graph.bindings[..first_binding]);
    for binding in &mut graph.bindings {
        if binding.record.kind == BindingKind::Collection
            && binding.role == Role::Provider
            && roles.contains(&binding.record.primary.type_id())
        {
            binding.role = Role::Enhancer;
        }
    }

    for binding in &graph.bindings {
        let record = &binding.record;
        let new = binding.id.0 as usize >= first_binding;
        let reported = |key: Key| roles.contains(&key.type_id()) && (new || !checked.contains(&key.type_id()));
        match record.kind {
            BindingKind::Collection => {
                let key = record_key(record);
                if !key.is_unqualified() && reported(key) {
                    errors.push(WiringError::QualifiedRoleContribution {
                        key: key.name(BindingKind::Collection),
                        module: graph.module_name(binding.origin),
                        at: record.location,
                    });
                }
            }
            BindingKind::Single => {
                for (key, at) in record.keys_located().filter(|&(key, _)| reported(key)) {
                    errors.push(WiringError::SingleRoleBinding {
                        key: key.name(BindingKind::Single),
                        module: graph.module_name(binding.origin),
                        at,
                    });
                }
            }
        }
    }
}

/// The role keys' types among `bindings`: those a mounted handler's transport reads,
/// `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>`, and the key of every
/// contribution with the enhancer role. Freezing gives that role to a contribution written
/// through `ModuleDef::enhancer`, which covers a transport with no mounted handler, and
/// `mark_role_contributions` only to one under a key already in the set; `assign_roles` gives it
/// to single bindings only. The set is the same before and after either pass.
pub(crate) fn role_types(graph: &Graph, bindings: &[FrozenBinding]) -> HashSet<TypeId> {
    let mut roles: HashSet<TypeId> =
        graph.handlers.iter().flat_map(|handler| handler.decl.role_keys).map(|key| key.type_id()).collect();
    roles.extend(
        bindings
            .iter()
            .filter(|binding| binding.record.kind == BindingKind::Collection && binding.role == Role::Enhancer)
            .map(|binding| binding.record.primary.type_id()),
    );
    roles
}

/// Marks the bindings an `EnhancerSpec` names by type, resolved against the controller module's
/// visibility, as enhancers. Runs before `needs_execution`.
///
/// Every other role is decided at freeze: `controller`, a contribution under a role key
/// (`mark_role_contributions`), or anything else as a provider. A lazy load re-runs this over
/// the base bindings, whose roles only ever move from provider to enhancer here.
pub(crate) fn assign_roles(graph: &mut Graph) {
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
/// bindings built per execution. A readiness check counts as a hook here: `connect` runs it,
/// and a binding built per call never reaches `connect`.
///
/// A binding with hooks built per execution is an `Auto` one the pass inferred so. An explicitly
/// execution-scoped binding carries neither hooks nor a check: the handle and the hook traits
/// both require a `HookCapable` scope.
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
                by_closure: record.by_closure(),
                module: graph.module_name(binding.origin),
                path: printed_path(graph, binding.id),
            });
        } else if binding.effective == Effective::PerExecution && (!record.hooks.is_empty() || record.ready.is_some()) {
            errors.push(WiringError::HooksOnPerExecution {
                binding: graph.key_name(binding.id),
                module: graph.module_name(binding.origin),
            });
        }
    }
}

/// Decides the scope of each enhancer a handler declares by closure, as `needs_execution` decides
/// a binding's, and records it in `Graph::closures`, where the pipeline reads it (§7):
///
/// | Declared | Reads execution data | Built |
/// |---|---|---|
/// | `Auto` | no | once, on first use |
/// | `Auto` | yes | per execution |
/// | `Singleton` | no | once, on first use |
/// | `Singleton` | yes | refused as `ClosureScopeViolation` |
/// | `PerExecution` | — | per execution |
/// | `Transient` | — | each time the pipeline obtains it |
///
/// A closure reads execution data through the reads `check_closures` looks for: `Ext`,
/// `ExecutionRef`, an execution input, or a binding that passes an execution need upward. Runs
/// after `needs_execution`, over the handlers this wiring added: a lazy load mounts none.
pub(crate) fn closure_scopes(graph: &mut Graph, declared: &Declared, errors: &mut Vec<WiringError>) {
    let mut decided = Vec::new();
    for handler in graph.handlers.iter().skip(declared.first_handler) {
        for dep in &handler.decl.enhancer_deps {
            let EnhancerDep::Closure(closure) = dep else { continue };
            let read = closure.dependencies.list.iter().find_map(|dependency| {
                execution_read(graph, handler.module, None, dependency).map(|steps| {
                    let mut path = vec![dependency_text(dependency)];
                    path.extend(steps);
                    path
                })
            });
            let effective = match closure.scope {
                ScopeKind::Auto if read.is_some() => Effective::PerExecution,
                ScopeKind::Auto => Effective::Singleton,
                ScopeKind::Singleton => {
                    if let Some(path) = read {
                        errors.push(WiringError::ClosureScopeViolation {
                            closure: format!(
                                "{}-level {} #{} of {} ({})",
                                closure.tier,
                                closure.role,
                                closure.position,
                                graph.handler_name(handler),
                                handler.decl.transport_name
                            ),
                            role: closure.role,
                            path,
                            at: closure.location,
                        });
                    }
                    Effective::Singleton
                }
                ScopeKind::PerExecution => Effective::PerExecution,
                ScopeKind::Transient => Effective::Transient,
            };
            decided.push((closure.id, effective));
        }
    }
    graph.closures.extend(decided);
}

/// A hook, readiness, module-hook or metadata closure runs where no execution exists, so one
/// that reads `Ext`, `ExecutionRef`, an execution input or a per-execution key is refused here
/// rather than failing at `connect` (§6.2). Enhancer closures are left to `closure_scopes`. A
/// read is refused whether or not it is optional: without an execution `Option<S>` propagates
/// `ExecutionRequired` rather than answering `None`.
///
/// A closure reading its own binding is left to `HooksOnPerExecution`, which already reports a
/// binding that hooks and a check cannot run on. One error per offending injection point.
pub(crate) fn check_closures(graph: &Graph, declared: &Declared, errors: &mut Vec<WiringError>) {
    for binding in graph.bindings.iter().skip(declared.first_binding) {
        let module = graph.module_name(binding.origin);
        let label = graph.label(binding.id);
        if let Some(ready) = &binding.record.ready {
            let closure = format!("readiness check of `{label}` in {module}");
            let site = Closure { module: binding.origin, owner: Some(binding.id), name: &closure, at: Some(ready.location) };
            site.check(graph, &ready.dependencies, errors);
        }
        for hook in &binding.record.hooks {
            let closure = format!("`{}` hook of `{label}` in {module}", hook.kind);
            let site = Closure { module: binding.origin, owner: Some(binding.id), name: &closure, at: Some(hook.location) };
            site.check(graph, &hook.dependencies, errors);
        }
    }
    for module in graph.modules.iter().skip(declared.first_module) {
        for hook in &module.hooks {
            let closure = format!("`{}` hook of module {}", hook.kind, module.name);
            let site = Closure { module: module.id, owner: None, name: &closure, at: Some(hook.location) };
            site.check(graph, &hook.dependencies, errors);
        }
    }
    for (module, name, dependencies) in &declared.meta {
        let closure = format!("metadata `{}` of {}", short_type_name(*name), graph.module_name(*module));
        let site = Closure { module: *module, owner: None, name: &closure, at: None };
        site.check(graph, dependencies, errors);
    }
}

/// A closure that runs outside any execution, as `check_closures` names it.
struct Closure<'a> {
    module: ModuleId,
    /// The binding the closure belongs to, `None` for a module hook or metadata.
    owner: Option<BindingId>,
    name: &'a str,
    at: Option<&'static Location<'static>>,
}

impl Closure<'_> {
    fn check(&self, graph: &Graph, dependencies: &Dependencies, errors: &mut Vec<WiringError>) {
        for dependency in &dependencies.list {
            if let Some(steps) = execution_read(graph, self.module, self.owner, dependency) {
                let mut path = vec![dependency_text(dependency)];
                path.extend(steps);
                errors.push(WiringError::ClosureNeedsExecution { closure: self.name.to_owned(), path, at: self.at });
            }
        }
    }
}

/// Whether a closure's injection point, resolved in `module`, reads execution data, with the
/// steps from what it reads to the read that introduces the need; `None` when it reads none. A
/// binding passes the need when `passes_execution` says so, except `owner`, the binding the
/// closure belongs to.
fn execution_read(
    graph: &Graph,
    module: ModuleId,
    owner: Option<BindingId>,
    dependency: &DependencyRecord,
) -> Option<Vec<String>> {
    let through = |id: BindingId| Some(id) != owner && graph.passes_execution(id);
    dependency.requirement.reads.iter().find_map(|read| match &read.kind {
        ReadKind::Extension(_) | ReadKind::Execution => Some(Vec::new()),
        ReadKind::Single(key) => match graph.lookup(module, *key) {
            Some(Visible::Input(input)) => Some(vec![format!("input `{}`", input.name(BindingKind::Single))]),
            Some(Visible::Binding(id)) if through(*id) => Some(execution_steps(graph, *id)),
            _ => None,
        },
        ReadKind::Collection(key) => {
            graph.collection(*key).iter().copied().find(|&id| through(id)).map(|id| execution_steps(graph, id))
        }
        ReadKind::Module => None,
    })
}

/// The steps from `id`, which passes an execution need upward, to the read of execution data
/// that introduces it: `AuditContext (execution) → Ext<CurrentUser> (field `user`)`.
fn execution_steps(graph: &Graph, id: BindingId) -> Vec<String> {
    let ids = execution_path(graph, id);
    let mut steps: Vec<String> = ids.iter().map(|&step| graph.scoped_label(step)).collect();
    if let Some(&last) = ids.last() {
        if let Some(edge) = graph.direct_execution_edge(last) {
            steps.push(graph.dependency_step(last, edge.dependency));
        }
    }
    steps
}

/// For each handler, every non-optional input on its reachable execution-scoped bindings that
/// the handler's transport does not seed, with the path from the handler to the service that
/// reads it, the handler's transport and the input's seeder. Each input is reported once per
/// handler and reading binding.
pub(crate) fn check_inputs(graph: &Graph, errors: &mut Vec<WiringError>) {
    for handler in &graph.handlers {
        for read in input_reads(graph, handler) {
            let Some(decl) = graph.inputs.get(&read.input) else { continue };
            if decl.seeder == handler.decl.transport {
                continue;
            }
            errors.push(WiringError::InputNotSeeded {
                handler: graph.handler_name(handler),
                transport: handler.decl.transport_name,
                input: read.input.name(BindingKind::Single),
                seeder: decl.seeder_name,
                path: read.path,
            });
        }
    }
}

/// One non-optional read of an execution input that a handler reaches, as [`input_reads`] finds
/// it.
pub(crate) struct InputRead {
    pub(crate) input: Key,
    /// The path from the handler to the injection point, the handler first:
    /// `UsersController::get (Http) → Audit (execution) → Dep<ClientAddr> (field `addr`)`.
    pub(crate) path: Vec<String>,
}

/// Every non-optional read of an execution input that `handler` reaches, once per input and
/// reading binding, whoever seeds it.
///
/// What a handler reaches: its controller, the global enhancers under its transport's role
/// keys, what its own parameters read (`HandlerSpec::dependencies`), the bindings its by-type
/// enhancers name, and what its enhancer closures built in an execution read. An input a
/// parameter or a closure reads directly is found there. The walk enters execution-scoped and
/// transient bindings only, since a singleton is built at `connect` with no execution, and one
/// reading an input has already failed the scope check.
pub(crate) fn input_reads(graph: &Graph, handler: &HandlerRecord) -> Vec<InputRead> {
    let mut walk = InputWalk::new(graph, handler);
    walk.run();
    walk.reads
}

struct InputWalk<'g> {
    graph: &'g Graph,
    handler: &'g HandlerRecord,
    /// The handler as the first step of a path: `UsersController::get_rpc (Rpc)`.
    head: String,
    seen: HashSet<(Key, Option<BindingId>)>,
    reads: Vec<InputRead>,
}

impl<'g> InputWalk<'g> {
    fn new(graph: &'g Graph, handler: &'g HandlerRecord) -> Self {
        let head = format!("{} ({})", graph.handler_name(handler), handler.decl.transport_name);
        InputWalk { graph, handler, head, seen: HashSet::new(), reads: Vec::new() }
    }

    fn run(&mut self) {
        let graph = self.graph;
        let handler = self.handler;
        let mut roots = vec![handler.controller];
        for key in handler.decl.role_keys {
            roots.extend(graph.collection(key).iter().copied());
        }
        self.read_roots(&handler.decl.dependencies, None, &mut roots);
        for dep in &handler.decl.enhancer_deps {
            match dep {
                EnhancerDep::Type(key) => {
                    if let Some(Visible::Binding(id)) = graph.lookup(handler.module, *key) {
                        roots.push(*id);
                    }
                }
                // A closure built once reads no execution data, or `closure_scopes` refused it.
                EnhancerDep::Closure(closure) if graph.closures.get(&closure.id) == Some(&Effective::Singleton) => {}
                EnhancerDep::Closure(closure) => {
                    self.read_roots(&closure.dependencies, Some("enhancer closure"), &mut roots);
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
                        path.push(graph.dependency_step(node, edge.dependency));
                        self.record(*input, Some(node), path);
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

    /// What `dependencies`, read inside the handler's execution, reach: each binding a single read
    /// resolves to and every contribution of a collection read, pushed onto `roots`, and each
    /// non-optional input read directly, recorded here. `via` names what reads them between the
    /// handler and the injection point, `None` for the handler's own parameters.
    fn read_roots(&mut self, dependencies: &Dependencies, via: Option<&str>, roots: &mut Vec<BindingId>) {
        let graph = self.graph;
        let module = self.handler.module;
        for dependency in &dependencies.list {
            for read in &dependency.requirement.reads {
                match &read.kind {
                    ReadKind::Single(key) => match graph.lookup(module, *key) {
                        Some(Visible::Input(input)) if !read.optional => {
                            let mut path = vec![self.head.clone()];
                            path.extend(via.map(str::to_owned));
                            path.push(dependency_text(dependency));
                            self.record(*input, None, path);
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

    fn record(&mut self, input: Key, reader: Option<BindingId>, path: Vec<String>) {
        if self.seen.insert((input, reader)) {
            self.reads.push(InputRead { input, path });
        }
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
/// reads execution data itself, or at an execution-scoped one, whose own injection points need
/// not read any.
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

/// `execution_path` as a report prints it: the binding, each step with its scope, then the
/// injection point that reads execution data, as in
/// `ReportService → AuditContext (execution) → Ext<CurrentUser>`.
fn printed_path(graph: &Graph, from: BindingId) -> Vec<String> {
    let ids = execution_path(graph, from);
    let mut steps: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(position, &id)| if position == 0 { graph.label(id) } else { graph.scoped_label(id) })
        .collect();
    if let Some(&last) = ids.last() {
        if let Some(edge) = graph.direct_execution_edge(last) {
            steps.push(graph.dependency_step(last, edge.dependency));
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
