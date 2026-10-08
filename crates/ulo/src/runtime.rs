//! Spawning without a runtime of the core's own: [`Spawn`], which with [`Timer`] makes a
//! [`Runtime`], the [`TaskHandle`] every runtime answers, and the [`RuntimeTask`] a runtime
//! adapter implements to build one.
//!
//! A handle means the same on every runtime: dropping it detaches the task, `abort` stops it, and
//! awaiting it answers a [`TaskEnd`]. A panic is caught inside the task by the core's own wrapper,
//! so it ends the task as `Panicked` whether the runtime would have reported it or re-raised it.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, ready};

use crate::lifecycle::run::CatchUnwind;
use crate::timer::{BoxFuture, Timer};

/// Starts tasks on a runtime's executor. With [`Timer`] it makes a [`Runtime`].
pub trait Spawn: Send + Sync + 'static {
    /// Starts `fut` as a task of its own. The task runs whether the handle is kept or not:
    /// dropping the handle detaches it. An implementation builds the handle with
    /// [`TaskHandle::launch`], which is what makes a panic end the task as `TaskEnd::Panicked`.
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle;
}

/// The app's clock and executor in one value, set with `AppBuilder::runtime`. Services read it as
/// `Dep<dyn Runtime>`, and `Dep<dyn Timer>` resolves to the same object. Implemented for anything
/// that implements both halves.
pub trait Runtime: Timer + Spawn {}

impl<R: Timer + Spawn + ?Sized> Runtime for R {}

/// How a spawned task ended, as its handle answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEnd {
    /// The future returned.
    Finished,
    /// The future was dropped before it returned: the task was aborted, or its runtime dropped it
    /// unfinished, as one shutting down does.
    Aborted,
    /// The future panicked. The panic was caught at the task's boundary and went no further.
    Panicked,
}

/// A runtime's own handle to one task, boxed inside a [`TaskHandle`]. A runtime adapter
/// implements it; code that spawns never sees it.
///
/// The future the runtime spawns is the one [`TaskHandle::launch`] hands it, which catches a panic
/// and records how the task's future ended before completing normally. An implementation reports
/// only whether the task is over, and the handle reads `Finished` or `Panicked` from the record,
/// or `Aborted` when nothing was recorded.
///
/// - [`abort`](Self::abort) asks the task to stop. It may return before the future has stopped;
///   [`poll_ended`](Self::poll_ended) reports when it has. On a task that has already ended it does
///   nothing, and calling it twice is the same as calling it once.
/// - [`poll_ended`](Self::poll_ended) is `Ready` once the future will never be polled again and has
///   been dropped: it completed, the abort took effect, or the runtime dropped it. Never before:
///   code waiting for a set of tasks to end relies on no part of any of them running afterwards.
///   Once `Ready`, it stays `Ready`.
/// - [`detach`](Self::detach) lets the task run on with nothing waiting for it. `TaskHandle` calls
///   it from its own drop, on a task in any state, and calls nothing after it. Detaching an
///   aborted task leaves it aborted.
///
/// On tokio the runtime's handle is a `JoinHandle<()>`: `abort` is `JoinHandle::abort`,
/// `poll_ended` polls the `JoinHandle`, and `detach` drops it. On smol the `Task<()>` is held in an
/// `Option`: `abort` takes it out to cancel it, and `detach` calls `Task::detach` on it if it is
/// still there. Dropping a smol task cancels it but can return while its future is still being
/// polled on another thread, so an `abort` meeting the rule above keeps the future
/// `Task::cancel` answers, polls it once at once, since `cancel` is an `async fn` that cancels on
/// its first poll, and `poll_ended` polls it to its end.
pub trait RuntimeTask: Send + 'static {
    fn abort(&mut self);
    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn detach(self: Box<Self>);
}

const RUNNING: u8 = 0;
const FINISHED: u8 = 1;
const PANICKED: u8 = 2;

/// A spawned task: [`abort`](Self::abort) to stop it, `.await` for how it ended, drop to let it
/// run on.
///
/// Dropping the handle detaches the task rather than stopping it, so a task spawned and forgotten
/// runs to its end. A task that must not outlive its owner is aborted by it, or spawned into a
/// `ulo_transport::TaskSet`, which aborts what it holds when dropped. Once the handle has answered,
/// it answers the same end each time it is polled.
pub struct TaskHandle {
    /// `None` only once `drop` has detached it.
    task: Mutex<Option<Box<dyn RuntimeTask>>>,
    /// `RUNNING` until the wrapper records `FINISHED` or `PANICKED`, before the spawned future
    /// completes.
    recorded: Arc<AtomicU8>,
    end: Option<TaskEnd>,
}

impl TaskHandle {
    /// Builds the handle for a task a runtime starts, from inside its [`Spawn::spawn`]. `start`
    /// receives `fut` wrapped, spawns what it receives, and answers the runtime's own handle to the
    /// task. The wrapper catches a panic in `fut` with `catch_unwind` at each poll and completes
    /// normally whether `fut` returned or panicked.
    pub fn launch<T, S>(fut: BoxFuture<'static, ()>, start: S) -> TaskHandle
    where
        T: RuntimeTask,
        S: FnOnce(BoxFuture<'static, ()>) -> T,
    {
        let recorded = Arc::new(AtomicU8::new(RUNNING));
        let record = Arc::clone(&recorded);
        let task = start(Box::pin(async move {
            let end = match CatchUnwind::boxed(fut).await {
                Ok(()) => FINISHED,
                Err(_payload) => PANICKED,
            };
            record.store(end, Ordering::Release);
        }));
        TaskHandle { task: Mutex::new(Some(Box::new(task))), recorded, end: None }
    }

    /// Asks the task to stop at its current await. The handle then answers `Aborted`, unless the
    /// future returned or panicked first, and does so only once the future has been dropped.
    pub fn abort(&self) {
        if self.end.is_some() {
            return;
        }
        if let Some(task) = self.task.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            task.abort();
        }
    }
}

impl Future for TaskHandle {
    type Output = TaskEnd;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<TaskEnd> {
        let this = self.get_mut();
        if let Some(end) = this.end {
            return Poll::Ready(end);
        }
        let task = this.task.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(task) = task.as_mut() {
            ready!(task.poll_ended(cx));
        }
        let end = match this.recorded.load(Ordering::Acquire) {
            FINISHED => TaskEnd::Finished,
            PANICKED => TaskEnd::Panicked,
            _ => TaskEnd::Aborted,
        };
        this.end = Some(end);
        Poll::Ready(end)
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap_or_else(PoisonError::into_inner).take() {
            task.detach();
        }
    }
}

/// Spawns `fut` on `runtime` and answers a handle that resolves to its value. The value is left
/// in a slot the handle reads once the task has ended, so no channel is involved; `abort` and
/// drop mean what they mean on [`TaskHandle`].
pub fn spawn_with<T, F, R>(runtime: &R, fut: F) -> ValueHandle<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
    R: Spawn + ?Sized,
{
    let slot = Arc::new(Mutex::new(None));
    let fill = Arc::clone(&slot);
    let task = runtime.spawn(Box::pin(async move {
        let value = fut.await;
        *fill.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
    }));
    ValueHandle { task, slot }
}

/// A spawned task that answers a value, as [`spawn_with`] starts it: `Ok` with the value, or `Err`
/// with `TaskEnd::Aborted` or `TaskEnd::Panicked`. Polled again after answering `Ok`, it answers
/// `Err(TaskEnd::Finished)`, the value having been taken.
pub struct ValueHandle<T> {
    task: TaskHandle,
    slot: Arc<Mutex<Option<T>>>,
}

impl<T> ValueHandle<T> {
    /// [`TaskHandle::abort`].
    pub fn abort(&self) {
        self.task.abort();
    }
}

impl<T> Future for ValueHandle<T> {
    type Output = Result<T, TaskEnd>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let end = ready!(Pin::new(&mut this.task).poll(cx));
        let value = this.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        Poll::Ready(match (end, value) {
            (TaskEnd::Finished, Some(value)) => Ok(value),
            (end, _) => Err(end),
        })
    }
}
