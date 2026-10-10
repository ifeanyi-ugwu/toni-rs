//! The scenarios, one public `async fn` each, taking the runtime by value. [`runtime_suite!`]
//! stamps a `#[test]` per scenario; a runtime crate calls one directly only to run it alone.
//!
//! [`runtime_suite!`]: crate::runtime_suite

use std::fmt::{self, Write as _};
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use futures::future::{Either, select};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};
use ulo::{TaskEnd, Timer};

pub mod app;
pub mod set;
pub mod task;
pub mod value;

/// How long a scenario waits for anything it expects before failing.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// How long a scenario waits for something it expects not to happen.
pub const SETTLE: Duration = Duration::from_millis(100);

/// How many tasks a set scenario spawns.
const TASKS: usize = 3;

/// `fut`'s output, or `None` once [`PATIENCE`] has passed on `timer`'s clock.
async fn within<T: Timer + ?Sized, F: Future>(timer: &T, fut: F) -> Option<F::Output> {
    within_for(timer, PATIENCE, fut).await
}

/// `fut`'s output, or `None` once `after` has passed on `timer`'s clock. `fut` is dropped then.
async fn within_for<T: Timer + ?Sized, F: Future>(timer: &T, after: Duration, fut: F) -> Option<F::Output> {
    match select(pin!(fut), timer.sleep(after)).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(_) => None,
    }
}

/// Counts its own drop, which is when the future holding it is dropped. The count is added only after
/// a blocking pause, so a handle answering while the drop is still under way reads it unchanged.
struct Dropped(Arc<AtomicUsize>);

impl Drop for Dropped {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(20));
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// What a panicking task panics with: a URL carrying a password, which the message must arrive
/// without.
const PANIC_MESSAGE: &str = "ulo-runtime-conformance: a deliberate panic inside a task, reaching postgres://suite:hunter2@db.invalid/app";

/// What a task's state panics with while its aborted future is dropped.
const DROP_PANIC_MESSAGE: &str = "ulo-runtime-conformance: a deliberate panic while an aborted task's future is dropped";

/// `end` is `Panicked` with [`PANIC_MESSAGE`], its password redacted.
fn assert_panicked_with_the_message(end: &TaskEnd) {
    let TaskEnd::Panicked(message) = end else {
        panic!("a task whose future panicked ended {end:?}");
    };
    let text = message.to_string();
    assert!(
        text.contains("a deliberate panic inside a task") && text.contains("postgres://[redacted]@db.invalid/app"),
        "the panic's message, redacted, did not arrive: {text:?}"
    );
    assert!(!text.contains("hunter2"), "the panic's message arrived unredacted: {text:?}");
}

/// The process's global `tracing` subscriber while the suite runs, keeping every `warn` and
/// `error` event's level and fields. A runtime drops an aborted future on a thread of its own, so
/// a thread-local subscriber would miss the event.
struct Recorder {
    events: Mutex<Vec<(Level, String)>>,
}

impl Recorder {
    /// The recorder, installed as the global default on first use. Fails the scenario when another
    /// global subscriber was installed first, since that one would receive the events instead.
    fn installed() -> &'static Recorder {
        static RECORDER: OnceLock<&'static Recorder> = OnceLock::new();
        RECORDER.get_or_init(|| {
            let recorder: &'static Recorder = Box::leak(Box::new(Recorder { events: Mutex::new(Vec::new()) }));
            tracing::subscriber::set_global_default(Forward(recorder))
                .expect("another global `tracing` subscriber was installed before the suite's");
            recorder
        })
    }

    /// The text of every `warn` event whose fields contain `needle`.
    fn warnings_containing(&self, needle: &str) -> Vec<String> {
        let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        events.iter().filter(|(level, text)| *level == Level::WARN && text.contains(needle)).map(|(_, text)| text.clone()).collect()
    }
}

/// The subscriber the global default holds, writing into the leaked [`Recorder`].
struct Forward(&'static Recorder);

impl Subscriber for Forward {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut text = Fields(String::new());
        event.record(&mut text);
        self.0.events.lock().unwrap_or_else(PoisonError::into_inner).push((*event.metadata().level(), text.0));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// An event's fields as `name=value` pairs.
struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}
