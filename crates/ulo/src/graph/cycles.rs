//! Import cycles (step 1) and dependency cycles (step 4), each printed as a full path.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::wiring::WiringError;
use crate::graph::register::Registry;
use crate::graph::{BindingId, Graph};
use crate::module::ModuleName;

/// Every import cycle the registration walk met, as the path of module names. `names` is the
/// registry's, by node.
pub(crate) fn import_cycles(registry: &Registry, names: &[ModuleName], errors: &mut Vec<WiringError>) {
    for cycle in &registry.import_cycles {
        let path = cycle.iter().filter_map(|&node| names.get(node).cloned()).collect();
        errors.push(WiringError::ImportCycle { path });
    }
}

/// A DFS over the resolved edges; each cycle printed as `A → B → C → A` with the module of each
/// step. Edges into collections count: every contribution is an edge. So does what a readiness
/// check reads, other than its own binding, since the check must pass before anything reading
/// the binding is built.
///
/// One cycle is printed per strongly connected component, starting at its smallest binding:
/// every elementary cycle would be exponential in the worst case, and one per tangle names
/// where to cut.
pub(crate) fn dependency_cycles(graph: &Graph, errors: &mut Vec<WiringError>) {
    let successors: Vec<Vec<usize>> = (0..graph.bindings.len())
        .map(|index| {
            let id = BindingId(index as u32);
            let mut deps = graph.construction_deps(id);
            deps.extend(graph.readiness_deps(id));
            deps.into_iter().map(|dep| dep.0 as usize).collect()
        })
        .collect();

    for component in strongly_connected(&successors) {
        let cyclic = component.len() > 1 || component.first().is_some_and(|&only| successors[only].contains(&only));
        if !cyclic {
            continue;
        }
        let path = cycle_through(&successors, &component)
            .into_iter()
            .map(|index| {
                let id = BindingId(index as u32);
                (graph.key_name(id), graph.module_name(graph.binding(id).origin))
            })
            .collect();
        errors.push(WiringError::Cycle { path });
    }
}

/// Tarjan's algorithm, iterative so a long dependency chain cannot exhaust the stack.
fn strongly_connected(successors: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let count = successors.len();
    let mut index = vec![UNVISITED; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;

    for root in 0..count {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        // Each frame is a node and the position of its next successor to follow.
        let mut calls: Vec<(usize, usize)> = vec![(root, 0)];

        while let Some(&(node, position)) = calls.last() {
            match successors[node].get(position) {
                Some(&successor) => {
                    if let Some(frame) = calls.last_mut() {
                        frame.1 += 1;
                    }
                    if successor >= count {
                        continue;
                    }
                    if index[successor] == UNVISITED {
                        index[successor] = next;
                        low[successor] = next;
                        next += 1;
                        stack.push(successor);
                        on_stack[successor] = true;
                        calls.push((successor, 0));
                    } else if on_stack[successor] {
                        low[node] = low[node].min(index[successor]);
                    }
                }
                None => {
                    calls.pop();
                    if let Some(&(parent, _)) = calls.last() {
                        low[parent] = low[parent].min(low[node]);
                    }
                    if low[node] == index[node] {
                        let mut component = Vec::new();
                        while let Some(member) = stack.pop() {
                            on_stack[member] = false;
                            component.push(member);
                            if member == node {
                                break;
                            }
                        }
                        components.push(component);
                    }
                }
            }
        }
    }
    components
}

/// The shortest cycle through the component's smallest member, closed back on it:
/// `[start, .., start]`.
fn cycle_through(successors: &[Vec<usize>], component: &[usize]) -> Vec<usize> {
    let Some(&start) = component.iter().min() else { return Vec::new() };
    let members: HashSet<usize> = component.iter().copied().collect();
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for &successor in &successors[node] {
            if !members.contains(&successor) {
                continue;
            }
            if successor == start {
                let mut path = vec![node];
                let mut current = node;
                while current != start {
                    match parent.get(&current) {
                        Some(&previous) => {
                            current = previous;
                            path.push(current);
                        }
                        None => break,
                    }
                }
                path.reverse();
                path.push(start);
                return path;
            }
            if !parent.contains_key(&successor) {
                parent.insert(successor, node);
                queue.push_back(successor);
            }
        }
    }
    vec![start, start]
}
