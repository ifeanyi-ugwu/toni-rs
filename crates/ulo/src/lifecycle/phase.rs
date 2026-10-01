//! The app's phases and what each allows (§9.5):
//!
//! | Phase | New executions | Singleton lookups | `load` |
//! |---|---|---|---|
//! | Running | allowed | allowed | allowed |
//! | Stopping (step 1) | allowed | allowed | refused, `LoadError::Closed` |
//! | Draining (steps 2–4) | refused, `Closed`; terminal executions allowed | allowed | refused |
//! | Destroying onward (steps 5–7) | refused | `LookupError::Closed` | refused |

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

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
    /// Set at step 4. The phase stays `Draining` until the destroy step begins, and a terminal
    /// execution opened in between would start after the drain it is meant to run inside.
    drain_ended: AtomicBool,
    /// Terminal executions refused by phase, for `Shutdown::terminal_skipped`.
    terminal_refused: AtomicUsize,
}

impl PhaseCell {
    pub(crate) fn new(initial: Phase) -> Self {
        PhaseCell { phase: Mutex::new(initial), drain_ended: AtomicBool::new(false), terminal_refused: AtomicUsize::new(0) }
    }

    pub(crate) fn get(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Moves to `to` if it is later than the current phase; never moves back.
    pub(crate) fn advance(&self, to: Phase) {
        let mut phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        if to > *phase {
            *phase = to;
        }
    }

    /// Ordinary executions until Draining; terminal ones until the drain's end. Each refused
    /// terminal execution is counted toward `Shutdown::terminal_skipped`, so call this once per
    /// `open_terminal`.
    pub(crate) fn allows_execution(&self, terminal: bool) -> bool {
        let phase = self.get();
        if !terminal {
            return phase < Phase::Draining;
        }
        let allowed = phase <= Phase::Draining && !self.drain_ended.load(Ordering::Acquire);
        if !allowed {
            self.terminal_refused.fetch_add(1, Ordering::AcqRel);
        }
        allowed
    }

    pub(crate) fn allows_singleton_lookup(&self) -> bool {
        self.get() < Phase::Destroying
    }

    pub(crate) fn allows_load(&self) -> bool {
        self.get() < Phase::Stopping
    }

    /// The drain's end (§9.5 step 4): from here a terminal execution is refused.
    pub(crate) fn end_drain(&self) {
        self.drain_ended.store(true, Ordering::Release);
    }

    pub(crate) fn terminal_refused(&self) -> usize {
        self.terminal_refused.load(Ordering::Acquire)
    }
}
