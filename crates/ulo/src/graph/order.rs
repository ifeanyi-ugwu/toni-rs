//! The one order every reader of the graph uses (§3.2, §9.2): collection order for collections,
//! and its stable topological refinement for the connect walk, hooks and readiness checks.
//! Close runs the exact reverse.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use crate::graph::register::{Import, Registry};
use crate::graph::{BindingId, Effective, Graph};

/// Depth-first post-order over imports from the root, imports in the order written: each
/// module's position becomes its `ModuleId`. An import that closes a cycle is skipped, so every
/// module is placed once.
pub(crate) fn collection_order(registry: &Registry) -> Vec<usize> {
    let count = registry.nodes.len();
    let mut order = Vec::with_capacity(count);
    if count == 0 {
        return order;
    }
    let mut visited = vec![false; count];
    // Each frame is a node and the position of the next import to follow.
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    visited[0] = true;
    while let Some(frame) = stack.last_mut() {
        let (node, next) = *frame;
        match registry.imports.get(node).and_then(|imports| imports.get(next)) {
            Some(import) => {
                frame.1 += 1;
                if let Import::New(child) = *import {
                    if child < count && !visited[child] {
                        visited[child] = true;
                        stack.push((child, 0));
                    }
                }
            }
            None => {
                order.push(node);
                stack.pop();
            }
        }
    }
    order
}

/// The singletons in a stable topological sort: among the bindings whose dependencies are
/// done, the smallest (module post-order index, declaration index) runs next, so the order is
/// the same on every run.
///
/// A singleton waits for every singleton it reads, through transients and aliases built at the
/// read, and for what its readiness check reads. Should a cycle survive, its members follow the
/// rest in id order rather than being dropped.
pub(crate) fn connect_order(graph: &Graph) -> Vec<BindingId> {
    let count = graph.bindings.len();
    let is_singleton: Vec<bool> = graph.bindings.iter().map(|binding| binding.effective == Effective::Singleton).collect();

    let mut waiting = vec![0usize; count];
    let mut dependents: Vec<Vec<BindingId>> = vec![Vec::new(); count];
    for binding in &graph.bindings {
        let id = binding.id;
        if !is_singleton[id.0 as usize] {
            continue;
        }
        for dep in singleton_deps(graph, &is_singleton, id) {
            waiting[id.0 as usize] += 1;
            dependents[dep.0 as usize].push(id);
        }
    }

    let mut ready: BinaryHeap<Reverse<BindingId>> = graph
        .bindings
        .iter()
        .filter(|binding| is_singleton[binding.id.0 as usize] && waiting[binding.id.0 as usize] == 0)
        .map(|binding| Reverse(binding.id))
        .collect();
    let mut order = Vec::new();
    let mut placed = vec![false; count];
    while let Some(Reverse(id)) = ready.pop() {
        order.push(id);
        placed[id.0 as usize] = true;
        for &dependent in &dependents[id.0 as usize] {
            let left = &mut waiting[dependent.0 as usize];
            *left -= 1;
            if *left == 0 {
                ready.push(Reverse(dependent));
            }
        }
    }
    order.extend(
        graph
            .bindings
            .iter()
            .map(|binding| binding.id)
            .filter(|id| is_singleton[id.0 as usize] && !placed[id.0 as usize]),
    );
    order
}

/// The singletons `id` must wait for, each once. A transient or an alias is built at the read,
/// so what it reads is waited for in its place; an execution-scoped binding is never built by
/// `connect`, and a singleton reading one has failed the scope pass.
fn singleton_deps(graph: &Graph, is_singleton: &[bool], id: BindingId) -> Vec<BindingId> {
    let mut deps = Vec::new();
    let mut seen = HashSet::new();
    let mut pending = graph.construction_deps(id);
    pending.extend(graph.readiness_deps(id));
    while let Some(dep) = pending.pop() {
        if dep == id || !seen.insert(dep) {
            continue;
        }
        if is_singleton[dep.0 as usize] {
            deps.push(dep);
        } else if graph.binding(dep).effective == Effective::Transient {
            pending.extend(graph.construction_deps(dep));
        }
    }
    deps
}
