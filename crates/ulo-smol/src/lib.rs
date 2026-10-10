//! The smol runtime for `ulo` (transports DESIGN §8): the core's [`Runtime`](ulo::Runtime) as
//! [`Smol`], spawning on an `async-executor` [`Executor`] the caller owns and runs, and timing
//! with `async-io`'s timers. No tokio in its tree.
//!
//! ```ignore
//! let executor = Arc::new(async_executor::Executor::new());
//! let app = App::builder(AppModule).runtime(ulo_smol::Smol::new(Arc::clone(&executor))).wire()?;
//! // `serve` runs inside the executor, which runs every task the app spawns while it waits.
//! async_io::block_on(executor.run(async { app.connect().await?.bind(server).listen().await?.serve(signal).await }))?;
//! ```

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

pub use async_executor::Executor;
use async_executor::FallibleTask;
use ulo::{BoxFuture, RuntimeTask, TaskHandle};

/// The app's runtime on one `async-executor` [`Executor`]: tasks are spawned on it from whichever
/// thread asks, and run on the threads that run it (`executor.run(..)`, smol's own pattern), so a
/// spawned task makes progress only while some thread does. Sleeps are `async-io` timers, which
/// `async-io`'s own reactor thread fires whichever executor waits on them. Set with
/// `AppBuilder::runtime(ulo_smol::Smol::new(executor))`, or given to a client built outside an
/// app.
///
/// The executor is the caller's, not a global one: the caller decides how many threads run it and
/// when they stop. A task spawned on an executor nothing runs again is never polled, and an
/// executor dropped with tasks in it drops them, each handle answering `Aborted`.
#[derive(Clone)]
pub struct Smol {
    executor: Arc<Executor<'static>>,
}

impl Smol {
    pub fn new(executor: Arc<Executor<'static>>) -> Smol {
        Smol { executor }
    }

    pub fn executor(&self) -> &Arc<Executor<'static>> {
        &self.executor
    }
}

impl fmt::Debug for Smol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Smol").finish_non_exhaustive()
    }
}

/// `now` is the system's monotonic clock, the one `async-io` measures its timers against.
impl ulo::Timer for Smol {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            async_io::Timer::after(d).await;
        })
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl ulo::Spawn for Smol {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |task| Task::Running(self.executor.spawn(task).fallible()))
    }
}

/// `async-task`'s handle as the core's `RuntimeTask`.
///
/// Dropping an `async-task` `Task` cancels it but returns at once, with the future possibly still
/// being polled on another thread or not yet dropped by the executor; `poll_ended` must not answer
/// until the future has been dropped. So `abort` takes `Task::cancel`'s future, which waits for
/// exactly that, and polls it once at once, since it marks the task cancelled on its first poll;
/// `poll_ended` then polls it to its end. `FallibleTask` answers `None` rather than panicking for
/// a task its executor dropped unrun.
enum Task {
    Running(FallibleTask<()>),
    Cancelling(Pin<Box<dyn Future<Output = Option<()>> + Send>>),
    Ended,
}

impl RuntimeTask for Task {
    fn abort(&mut self) {
        let Task::Running(task) = std::mem::replace(self, Task::Ended) else { return };
        let mut cancel: Pin<Box<dyn Future<Output = Option<()>> + Send>> = Box::pin(task.cancel());
        if cancel.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending() {
            *self = Task::Cancelling(cancel);
        }
    }

    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let ended = match self {
            Task::Running(task) => Pin::new(task).poll(cx).map(|_| ()),
            Task::Cancelling(cancel) => cancel.as_mut().poll(cx).map(|_| ()),
            Task::Ended => Poll::Ready(()),
        };
        if ended.is_ready() {
            *self = Task::Ended;
        }
        ended
    }

    /// A cancelled task is already stopping; dropping its `cancel` future leaves it to.
    fn detach(self: Box<Self>) {
        if let Task::Running(task) = *self {
            task.detach();
        }
    }
}
