//! The scenarios, one public `async fn` each, taking the runtime by value. [`runtime_suite!`]
//! stamps a `#[test]` per scenario; a runtime crate calls one directly only to run it alone.
//!
//! [`runtime_suite!`]: crate::runtime_suite

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::future::{Either, select};
use ulo::Timer;

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
