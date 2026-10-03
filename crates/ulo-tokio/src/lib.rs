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
/// `"CTRL_CLOSE"`). A handler that cannot be installed is skipped and the others still resolve it;
/// with none installed, or on another platform, the future never resolves.
///
/// The handlers are installed when the future is first polled, inside the tokio runtime that
/// polls it, so the function may be called anywhere. On Unix tokio keeps a handler installed for
/// the rest of the process, so a second SIGINT during the shutdown sequence does not end it.
pub fn shutdown_signal() -> impl Future<Output = Signal> + Send + 'static {
    os::shutdown_signal()
}

#[cfg(unix)]
mod os {
    use tokio::signal::unix::{SignalKind, signal};
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        let sigterm = signal(SignalKind::terminate()).ok();
        let sigint = signal(SignalKind::interrupt()).ok();
        let terminate = async move {
            if let Some(mut stream) = sigterm
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        let interrupt = async move {
            if let Some(mut stream) = sigint
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = terminate => Signal::new("SIGTERM"),
            () = interrupt => Signal::new("SIGINT"),
        }
    }
}

#[cfg(windows)]
mod os {
    use tokio::signal::windows::{ctrl_c, ctrl_close};
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        let on_ctrl_c = ctrl_c().ok();
        let on_ctrl_close = ctrl_close().ok();
        let ctrl_c = async move {
            if let Some(mut stream) = on_ctrl_c
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        let ctrl_close = async move {
            if let Some(mut stream) = on_ctrl_close
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = ctrl_c => Signal::new("CTRL_C"),
            () = ctrl_close => Signal::new("CTRL_CLOSE"),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        std::future::pending().await
    }
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
