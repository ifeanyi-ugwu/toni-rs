//! The wiring pass: the six steps of §10.1, collecting every error and stopping at none. Steps
//! that depend on a missing piece skip only the affected edges, so one missing binding does not
//! hide unrelated errors.

use std::sync::Arc;

use crate::error::LoadRefusal;
use crate::error::wiring::{WiringError, WiringErrors};
use crate::graph::register::Registry;
use crate::graph::{BindingId, Graph, ModuleId};
use crate::module::Module;
use crate::testing::TestPlan;
use crate::timer::Timer;

/// What the wiring pass needs to know about the app beyond its modules.
pub(crate) struct WireEnv {
    /// When set, bound under `dyn Timer` as a value in the core's own global module.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    /// The builder knobs that were set: each is a wiring error without a `Timer` (step 6).
    pub(crate) knobs_set: Vec<&'static str>,
}

/// Registers the root, freezes the graph and runs every check. `plan` carries a test's
/// overrides and module replacements.
pub(crate) fn wire(root: Box<dyn Module>, env: &WireEnv, plan: Option<TestPlan>) -> Result<Graph, WiringErrors> {
    todo!()
}

/// A lazily loaded module wired against the frozen graph: every error collected, and every
/// refusal of §8.6 checked before any of it is built.
pub(crate) fn wire_lazy(base: &Graph, module: Box<dyn Module>, env: &WireEnv) -> Result<LazyWiring, LazyFailure> {
    todo!()
}

pub(crate) struct LazyWiring {
    /// The base graph extended with the module and the imports it brought.
    pub(crate) graph: Graph,
    pub(crate) module: ModuleId,
    /// The new singletons in connect order, for `load` to build and check.
    pub(crate) singletons: Vec<BindingId>,
}

pub(crate) enum LazyFailure {
    Wiring(WiringErrors),
    Refused(LoadRefusal),
}

/// Assigns ids in collection order, requalifies keyed exports, moves recorded `try_value`
/// failures into `errors` (redacted), and applies the test plan's overrides (step 2).
pub(crate) fn freeze(registry: Registry, env: &WireEnv, plan: Option<TestPlan>, errors: &mut Vec<WiringError>) -> Graph {
    todo!()
}

/// Step 1: import cycles, re-exports a module cannot see unambiguously, inputs declared inside
/// a keyed module.
pub(crate) fn check_modules(registry: &Registry, graph: &Graph, errors: &mut Vec<WiringError>) {
    todo!()
}

/// Step 2: duplicate single bindings, single/collection mixes, aliases pointing at nothing,
/// two readiness checks on one binding.
pub(crate) fn check_bindings(graph: &Graph, errors: &mut Vec<WiringError>) {
    todo!()
}

/// Step 6: a `Timer` wherever an explicit bound is written, a readiness `.timeout` or
/// `.attempt_timeout`, a hook's or constructor's `After(..)`, or a builder knob.
pub(crate) fn check_environment(graph: &Graph, env: &WireEnv, errors: &mut Vec<WiringError>) {
    todo!()
}
