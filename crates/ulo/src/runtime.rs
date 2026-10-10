//! Spawning without a runtime of the core's own: [`Spawn`], which with [`Timer`] makes a
//! [`Runtime`], the [`TaskHandle`] every runtime answers, and the [`RuntimeTask`] a runtime
//! adapter implements to build one.
//!
//! A handle means the same on every runtime: dropping it detaches the task, `abort` stops it, and
//! awaiting it answers a [`TaskEnd`]. A panic is caught inside the task by the core's own wrapper,
//! so it ends the task as `Panicked`, carrying the panic's message, whether the runtime would have
//! reported it or re-raised it. The app's runtime, `Dep<dyn Runtime>` and `AppHandle::runtime()`,
//! is the core's wrapper around the one given, which redacts that message with the app's secrets.

use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use crate::lifecycle::run::CatchUnwind;
use crate::redact::{Redacted, Redactor, SecretRegistry, redact_panic};
use crate::timer::{BoxFuture, Timer};

/// Starts tasks on a runtime's executor. With [`Timer`] it makes a [`Runtime`].
pub trait Spawn: Send + Sync + 'static {
    /// Starts `fut` as a task of its own. The task runs whether the handle is kept or not:
    /// dropping the handle detaches it. An implementation builds the handle with
    /// [`TaskHandle::launch`], which is what makes a panic end the task as `TaskEnd::Panicked`.
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle;
}

/// The app's clock and executor in one value, set with `AppBuilder::runtime`. Services read it as
/// `Dep<dyn Runtime>`, and `Dep<dyn Timer>` resolves to the same object; after a later
/// `AppBuilder::timer`, `Dep<dyn Runtime>` spawns on the runtime and times with that timer. Either
/// way the object is the app's wrapper around the runtime given, which redacts a spawned task's
/// panic with the app's secrets. Implemented for anything that implements both halves.
pub trait Runtime: Timer + Spawn {}

impl<R: Timer + Spawn + ?Sized> Runtime for R {}

/// What `Dep<dyn Runtime>` resolves to: a runtime's spawning with the app's clock, the runtime's own
/// unless `AppBuilder::timer` replaced it, and the app's redaction for the panic of a task spawned
/// through it.
pub(crate) struct Clocked {
    spawner: Arc<dyn Runtime>,
    timer: Arc<dyn Timer>,
    redactor: Redactor,
}

impl Clocked {
    pub(crate) fn new(spawner: Arc<dyn Runtime>, timer: Arc<dyn Timer>, redactor: Redactor) -> Clocked {
        Clocked { spawner, timer, redactor }
    }
}

impl Timer for Clocked {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.timer.sleep(d)
    }

    fn now(&self) -> Instant {
        self.timer.now()
    }
}

/// The task's future goes to the runtime under a wrapper of the app's own, which catches a panic
/// and redacts it with the app's secrets; the handle the runtime answers reads that wrapper's
/// record. The runtime's wrapper around it then sees the future complete and never a panic.
impl Spawn for Clocked {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        let (launched, recorded) = Launched::new(fut, Some(self.redactor.clone()));
        self.spawner.spawn(Box::pin(launched)).reading(recorded)
    }
}

/// How a spawned task ended, as its handle answers it. Cloning shares a panic's message.
#[derive(Clone, Debug)]
pub enum TaskEnd {
    /// The future returned.
    Finished,
    /// The future was dropped before it returned: the task was aborted, or its runtime dropped it
    /// unfinished, as one shutting down does. A panic raised while the future is being dropped
    /// leaves the end `Aborted` and is logged at `warn`.
    Aborted,
    /// The future panicked. The panic was caught at the task's boundary and went no further; the
    /// payload, converted to a message, is redacted as `PanicRecovered`'s is. A task spawned
    /// through the app's runtime, `Dep<dyn Runtime>` or `AppHandle::runtime()`, has every secret
    /// the app registered replaced; one spawned on a runtime outside any app has only the
    /// userinfo strip.
    Panicked(Arc<Redacted>),
}

impl TaskEnd {
    pub fn is_finished(&self) -> bool {
        matches!(self, TaskEnd::Finished)
    }

    pub fn is_aborted(&self) -> bool {
        matches!(self, TaskEnd::Aborted)
    }

    pub fn is_panicked(&self) -> bool {
        matches!(self, TaskEnd::Panicked(_))
    }
}

/// A runtime's own handle to one task, boxed inside a [`TaskHandle`]. A runtime adapter
/// implements it; code that spawns never sees it.
///
/// The future the runtime spawns is the one [`TaskHandle::launch`] hands it, which catches a panic
/// and records how the task's future ended before completing normally. A panic raised while that
/// future is dropped is caught inside it too, so the runtime never sees one. An implementation
/// reports only whether the task is over, and the handle reads `Finished` or `Panicked` from the
/// record, or `Aborted` when nothing was recorded.
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

/// How the task's future ended, as the wrapper records it before the spawned future completes.
/// Nothing recorded means the future was dropped unfinished.
enum Recorded {
    Finished,
    Panicked(Redacted),
}

/// A spawned task: [`abort`](Self::abort) to stop it, `.await` for how it ended, drop to let it
/// run on.
///
/// Dropping the handle detaches the task, which runs to its end, whereas dropping a
/// `ulo_transport::TaskSet` aborts every task in it. Once the handle has answered, it answers the
/// same end each time it is polled.
pub struct TaskHandle {
    /// `None` only once `drop` has detached it.
    task: Mutex<Option<Box<dyn RuntimeTask>>>,
    /// Empty until the wrapper records an end, before the spawned future completes.
    recorded: Arc<Mutex<Option<Recorded>>>,
    /// The end answered, kept to answer again.
    end: Option<TaskEnd>,
}

impl TaskHandle {
    /// Builds the handle for a task a runtime starts, from inside its [`Spawn::spawn`]. `start`
    /// receives `fut` wrapped, spawns what it receives, and answers the runtime's own handle to the
    /// task. The wrapper catches a panic in `fut` with `catch_unwind` at each poll and completes
    /// normally whether `fut` returned or panicked; a panic while `fut` is dropped is caught as
    /// well and logged at `warn`.
    pub fn launch<T, S>(fut: BoxFuture<'static, ()>, start: S) -> TaskHandle
    where
        T: RuntimeTask,
        S: FnOnce(BoxFuture<'static, ()>) -> T,
    {
        let (launched, recorded) = Launched::new(fut, None);
        let task = start(Box::pin(launched));
        TaskHandle { task: Mutex::new(Some(Box::new(task))), recorded, end: None }
    }

    /// The handle reading `recorded`, the record of a wrapper inside the future the runtime was
    /// given, in place of the record of the runtime's own wrapper around it.
    fn reading(mut self, recorded: Arc<Mutex<Option<Recorded>>>) -> TaskHandle {
        self.recorded = recorded;
        self
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
        if let Some(end) = &this.end {
            return Poll::Ready(end.clone());
        }
        let task = this.task.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(task) = task.as_mut() {
            ready!(task.poll_ended(cx));
        }
        let end = match this.recorded.lock().unwrap_or_else(PoisonError::into_inner).take() {
            Some(Recorded::Finished) => TaskEnd::Finished,
            Some(Recorded::Panicked(message)) => TaskEnd::Panicked(Arc::new(message)),
            None => TaskEnd::Aborted,
        };
        Poll::Ready(this.end.insert(end).clone())
    }
}

/// The future a runtime spawns for a task: the task's own under the core's panic catch, which
/// records how it ended before completing.
struct Launched {
    /// `None` once it has completed, its future dropped.
    fut: Option<CatchUnwind<dyn Future<Output = ()> + Send>>,
    record: Arc<Mutex<Option<Recorded>>>,
    /// The app's secrets, for a task spawned through the app's runtime; `None` strips userinfo
    /// alone.
    redactor: Option<Redactor>,
}

impl Launched {
    fn new(fut: BoxFuture<'static, ()>, redactor: Option<Redactor>) -> (Launched, Arc<Mutex<Option<Recorded>>>) {
        let recorded = Arc::new(Mutex::new(None));
        (Launched { fut: Some(CatchUnwind::boxed(fut)), record: Arc::clone(&recorded), redactor }, recorded)
    }
}

fn redacted(redactor: Option<&Redactor>, payload: Box<dyn Any + Send>) -> Redacted {
    match redactor {
        Some(redactor) => redactor.panic(payload),
        None => redact_panic(&SecretRegistry::default(), payload),
    }
}

impl Future for Launched {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let Some(fut) = this.fut.as_mut() else {
            return Poll::Ready(());
        };
        let end = match ready!(Pin::new(fut).poll(cx)) {
            Ok(()) => Recorded::Finished,
            Err(payload) => Recorded::Panicked(redacted(this.redactor.as_ref(), payload)),
        };
        // Dropped before the end is recorded, so a handle reading the end finds the future gone.
        drop_caught(this.fut.take(), this.redactor.as_ref(), "after it ended");
        *this.record.lock().unwrap_or_else(PoisonError::into_inner) = Some(end);
        Poll::Ready(())
    }
}

/// An abort, or a runtime dropping the task unfinished, drops the future here, recording nothing,
/// so the handle answers `Aborted`.
impl Drop for Launched {
    fn drop(&mut self) {
        drop_caught(self.fut.take(), self.redactor.as_ref(), "unfinished, so the task ends `Aborted`");
    }
}

/// Drops a task's future inside `catch_unwind`. A panic there would otherwise reach the runtime:
/// tokio reports it through a `JoinHandle` the handle does not read, and `async-task`, under smol,
/// aborts the process.
fn drop_caught(fut: Option<CatchUnwind<dyn Future<Output = ()> + Send>>, redactor: Option<&Redactor>, when: &str) {
    if let Some(fut) = fut
        && let Err(payload) = catch_unwind(AssertUnwindSafe(move || drop(fut)))
    {
        let message = redacted(redactor, payload);
        tracing::warn!(%message, "a task's future panicked while it was dropped {when}");
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
/// drop mean what they mean on [`TaskHandle`]. Polling after the value was taken answers
/// `Err(TaskEnd::Finished)`.
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
/// with `TaskEnd::Aborted` or `TaskEnd::Panicked`. Polling after the value was taken answers
/// `Err(TaskEnd::Finished)`.
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
