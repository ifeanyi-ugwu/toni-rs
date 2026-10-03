//! The tokio runtime for `ulo` (transports DESIGN §8): the core's [`Timer`], the OS signal future
//! [`shutdown_signal`] for `serve`, and [`spawn`] and [`spawn_in`].
//!
//! ```ignore
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?;
//! // ..
//! app.serve(ulo_tokio::shutdown_signal()).await?;
//! ```

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use ulo::{BoxFuture, ExecutionRef, Signal};

/// The app's clock on tokio: sleeps through `tokio::time`, and `now()` through
/// `tokio::time::Instant::now().into_std()`, so a paused test clock (`tokio::time::pause`) drives
/// deadlines and sleeps together, as the core requires of one `Timer`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timer;

impl ulo::Timer for Timer {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(d))
    }

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
    }
}

/// Resolves on the first shutdown signal the OS delivers: SIGINT or SIGTERM on Unix, Ctrl-C or
/// Ctrl-Close on Windows, as a `Signal` named after it (`"SIGTERM"`, `"SIGINT"`, `"CTRL_C"`,
/// `"CTRL_CLOSE"`). A handler that cannot be installed is skipped and the others still resolve it.
pub fn shutdown_signal() -> impl Future<Output = Signal> + Send + 'static {
    async { todo!("select over the platform's signal streams; name the `Signal` after the one that fired") }
}

/// Spawns `fut` on the current tokio runtime.
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(fut)
}

/// Spawns `fut` holding `exec` for the task's lifetime, so the execution's cache and its
/// execution-scoped instances stay alive while the task runs, and stops the task when the
/// execution is cancelled: the handle answers `None` then, `Some` with the output otherwise.
///
/// The drain tracks executions, not tasks; a task that should hold shutdown open runs inside one
/// this way rather than detached.
pub fn spawn_in<F>(exec: &ExecutionRef, fut: F) -> JoinHandle<Option<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let exec = exec.clone();
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = exec.cancelled() => None,
            out = fut => Some(out),
        }
    })
}
