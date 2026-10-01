//! The app's phases and what each allows (§9.5):
//!
//! | Phase | New executions | Singleton lookups | `load` |
//! |---|---|---|---|
//! | Running | allowed | allowed | allowed |
//! | Stopping (step 1) | allowed | allowed | refused, `LoadError::Closed` |
//! | Draining (steps 2–4) | refused, `Closed`; terminal executions allowed | allowed | refused |
//! | Destroying onward (steps 5–7) | refused | `LookupError::Closed` | refused |

use std::sync::Mutex;

/// Ordered: every transition moves forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    Wired,
    /// The connect walk; a singleton lookup for one not yet built is `NotReady`.
    Connecting,
    Running,
    /// The before-shutdown stage; traffic is normal and `is_draining()` is false.
    Stopping,
    /// From stop-accepting to the drain's end.
    Draining,
    /// From the first destroy hook on.
    Destroying,
    Closed,
}

pub(crate) struct PhaseCell {
    phase: Mutex<Phase>,
}

impl PhaseCell {
    pub(crate) fn new(initial: Phase) -> Self {
        PhaseCell { phase: Mutex::new(initial) }
    }

    pub(crate) fn get(&self) -> Phase {
        todo!()
    }

    /// Moves to `to` if it is later than the current phase; never moves back.
    pub(crate) fn advance(&self, to: Phase) {
        todo!()
    }

    pub(crate) fn allows_execution(&self, terminal: bool) -> bool {
        todo!()
    }

    pub(crate) fn allows_singleton_lookup(&self) -> bool {
        todo!()
    }

    pub(crate) fn allows_load(&self) -> bool {
        todo!()
    }
}
