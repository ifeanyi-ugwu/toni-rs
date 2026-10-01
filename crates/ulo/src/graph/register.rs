//! The registration walk: calls `Module::register` once per identity, follows imports in the
//! order written, and computes collection order (§3.2, §10.1 step 1).

use std::collections::HashMap;

use crate::graph::{Graph, ModuleId};
use crate::module::def::ModuleNode;
use crate::module::{Module, ModuleIdentity};
use crate::testing::TestPlan;

/// Every module reached from a root, before freezing.
pub(crate) struct Registry {
    /// In first-reached order; `post_order` maps them to collection order.
    pub(crate) nodes: Vec<ModuleNode>,
    pub(crate) by_identity: HashMap<ModuleIdentity, usize>,
    /// Per node, its imports as node indices, in the order written. For a lazy registration an
    /// import of an already-frozen identity is `Import::Existing`.
    pub(crate) imports: Vec<Vec<Import>>,
    /// Depth-first post-order over imports from the root: collection order.
    pub(crate) post_order: Vec<usize>,
    /// Each import cycle, as the node path from the first module back to itself.
    pub(crate) import_cycles: Vec<Vec<usize>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Import {
    New(usize),
    Existing(ModuleId),
}

/// Registers `root` and everything it imports. Two imports of an equal identity are one module
/// and `register` runs once for it. A `TestPlan`'s `replace_module` swaps a module by identity
/// before its `register` runs.
pub(crate) fn register_all(root: Box<dyn Module>, plan: Option<&TestPlan>) -> Registry {
    todo!()
}

/// Registers a lazily loaded module against a frozen graph. An import whose identity the graph
/// already holds links to it and is not registered again.
pub(crate) fn register_lazy(base: &Graph, module: Box<dyn Module>) -> Registry {
    todo!()
}
