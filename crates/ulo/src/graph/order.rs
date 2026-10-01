//! The one order every reader of the graph uses (§3.2, §9.2): collection order for collections,
//! and its stable topological refinement for the connect walk, hooks and readiness checks.
//! Close runs the exact reverse.

use crate::graph::register::Registry;
use crate::graph::{BindingId, Graph};

/// Depth-first post-order over imports from the root, imports in the order written: each
/// module's position becomes its `ModuleId`.
pub(crate) fn collection_order(registry: &Registry) -> Vec<usize> {
    todo!()
}

/// The singletons in a stable topological sort: among the bindings whose dependencies are
/// done, the smallest (module post-order index, declaration index) runs next, so the order is
/// the same on every run.
pub(crate) fn connect_order(graph: &Graph) -> Vec<BindingId> {
    todo!()
}
