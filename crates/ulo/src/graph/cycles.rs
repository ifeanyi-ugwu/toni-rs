//! Import cycles (step 1) and dependency cycles (step 4), each printed as a full path.

use crate::error::wiring::WiringError;
use crate::graph::Graph;
use crate::graph::register::Registry;

/// Every import cycle the registration walk met, as the path of module names.
pub(crate) fn import_cycles(registry: &Registry, errors: &mut Vec<WiringError>) {
    todo!()
}

/// A DFS over the resolved edges; each cycle printed as `A → B → C → A` with the module of each
/// step. Edges into collections count: every contribution is an edge.
pub(crate) fn dependency_cycles(graph: &Graph, errors: &mut Vec<WiringError>) {
    todo!()
}
