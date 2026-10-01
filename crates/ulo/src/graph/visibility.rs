//! Step 3: the visibility tables and every site resolved against its module's table (§8.2,
//! §10.1 step 3).

use crate::error::wiring::WiringError;
use crate::graph::{Graph, ModuleId};
use crate::key::Key;

/// Fills `graph.visibility`: each module's own bindings, its direct imports' exports and the
/// globals' exports, a key with two sources recorded as `Visible::Ambiguous`.
pub(crate) fn build_tables(graph: &mut Graph) {
    todo!()
}

/// Resolves every site of every binding, hook closure, readiness check, enhancer declaration
/// and metadata value into `Edge`s, and reports missing keys (with the site, the key and the
/// module) and ambiguous keys (naming every source module). A missing key whose name equals a
/// bound key's name up to a trailing `+ core::marker::Send + core::marker::Sync` names both
/// spellings.
pub(crate) fn resolve_sites(graph: &mut Graph, errors: &mut Vec<WiringError>) {
    todo!()
}

/// The visible binding whose key prints like `missing` up to the `Send + Sync` suffix, for the
/// hint on a missing dependency.
pub(crate) fn near_spelling(graph: &Graph, module: ModuleId, missing: Key) -> Option<Key> {
    todo!()
}
