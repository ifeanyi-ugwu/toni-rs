//! The conformance suite every `ulo` runtime runs: what a [`TaskHandle`](ulo::TaskHandle) answers
//! after a return, an abort, a panic and a drop, what [`spawn_with`](ulo::spawn_with) and a
//! [`TaskSet`](ulo_transport::TaskSet) answer over it, and the app binding the runtime as one
//! object. One scenario list, stamped per runtime by [`runtime_suite!`] from a [`Harness`] the
//! runtime crate implements in its `tests/`.
//!
//! ```ignore
//! // crates/ulo-tokio/tests/conformance.rs
//! struct OnTokio;
//!
//! impl ulo_runtime_conformance::Harness for OnTokio {
//!     type Runtime = ulo_tokio::Tokio;
//!     fn runtime() -> ulo_tokio::Tokio { ulo_tokio::Tokio }
//!     fn block_on<F: Future>(fut: F) -> F::Output { /* a runtime of its own per test */ }
//! }
//!
//! ulo_runtime_conformance::runtime_suite!(OnTokio);
//! ```
//!
//! The suite names no runtime and depends on none. Every wait a scenario makes is bounded through
//! the runtime's own `Timer`, so a runtime that loses a task fails the scenario after
//! [`cases::PATIENCE`] instead of hanging it. A scenario that observes a panic causes one inside a
//! task, and the runtime's panic hook prints it to stderr while the scenario passes.

use std::future::Future;

use ulo::Runtime;

pub mod cases;

/// One runtime as the suite drives it.
pub trait Harness {
    type Runtime: Runtime;

    /// A runtime value; each scenario takes its own.
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks
    /// the runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;
}

/// Stamps one `#[test]` per scenario in [`cases`] for the [`Harness`] given.
#[macro_export]
macro_rules! runtime_suite {
    ($harness:ty $(,)?) => {
        $crate::runtime_suite!(@cases $harness;
            a_returned_future_is_finished => task::finished,
            a_dropped_handle_detaches => task::detached,
            abort_is_aborted_once_the_future_is_dropped => task::aborted,
            a_panic_is_panicked_and_goes_no_further => task::panicked,
            an_end_is_kept_and_abort_after_it_changes_nothing => task::end_kept,
            spawn_with_answers_the_value => value::answered,
            spawn_with_answers_an_abort => value::aborted,
            spawn_with_answers_a_panic => value::panicked,
            abort_all_aborts_every_task => set::abort_all,
            join_all_returns_once_every_task_has_ended => set::join_all,
            a_dropped_set_aborts_its_tasks => set::dropped,
            the_app_binds_the_runtime_as_one_object => app::one_object,
        );
    };
    (@cases $harness:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                <$harness as $crate::Harness>::block_on($crate::cases::$module::$case(
                    <$harness as $crate::Harness>::runtime(),
                ));
            }
        )*
    };
}
