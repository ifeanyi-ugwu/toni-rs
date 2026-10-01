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

use crate::error::wiring::WiringError;
use crate::graph::{BindingId, Graph};

/// Marks enhancer roles: bindings named by type in an `EnhancerSpec`, and contributions under a
/// role key of a transport that has handlers. Runs before `needs_execution`.
pub(crate) fn assign_roles(graph: &mut Graph) {
    todo!()
}

/// Sets `needs_execution` and `effective` on every binding, after the cycle check.
pub(crate) fn needs_execution(graph: &mut Graph) {
    todo!()
}

/// Scope violations with the path that introduces the execution dependency, and hooks on
/// bindings inferred per-execution.
pub(crate) fn check_scopes(graph: &Graph, errors: &mut Vec<WiringError>) {
    todo!()
}

/// For each handler, every non-optional input on its reachable execution-scoped bindings that
/// the handler's transport does not seed, with the path from the handler to the service that
/// reads it, the handler's transport and the input's seeder.
pub(crate) fn check_inputs(graph: &Graph, errors: &mut Vec<WiringError>) {
    todo!()
}

/// The dependency path from `from` to the first binding reading execution data, for a report.
pub(crate) fn execution_path(graph: &Graph, from: BindingId) -> Vec<BindingId> {
    todo!()
}
