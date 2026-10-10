//! A lone `TaskHandle`: the end it answers after a return, an abort and a panic, and what dropping
//! it does.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::channel::oneshot;
use ulo::{Runtime, TaskEnd};

use super::{DROP_PANIC_MESSAGE, Dropped, PANIC_MESSAGE, Recorder, assert_panicked_with_the_message, within};

/// A future that returns ends the task `Finished`, after it ran.
pub async fn finished<R: Runtime>(rt: R) {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let task = rt.spawn(Box::pin(async move { flag.store(true, Ordering::SeqCst) }));
    let end = within(&rt, task).await.expect("a task that returns at once did not end");
    assert!(end.is_finished(), "a task whose future returned ended {end:?}");
    assert!(ran.load(Ordering::SeqCst), "the handle answered `Finished` before the future ran");
}

/// Dropping the handle detaches the task, which runs to its end.
pub async fn detached<R: Runtime>(rt: R) {
    let (open, gate) = oneshot::channel::<()>();
    let (report, done) = oneshot::channel::<()>();
    let task = rt.spawn(Box::pin(async move {
        let _ = gate.await;
        let _ = report.send(());
    }));
    drop(task);
    // Opened only after the drop, so the task is waiting on it, or not yet started, when its
    // handle goes.
    let _ = open.send(());
    match within(&rt, done).await {
        Some(Ok(())) => {}
        Some(Err(_)) => panic!("dropping the handle stopped the task: its future was dropped unfinished"),
        None => panic!("the task did not reach its end after its handle was dropped"),
    }
}

/// `abort` at an await ends the task `Aborted`, answered only once its future has been dropped.
pub async fn aborted<R: Runtime>(rt: R) {
    let drops = Arc::new(AtomicUsize::new(0));
    let guard = Dropped(Arc::clone(&drops));
    let (report, started) = oneshot::channel::<()>();
    let task = rt.spawn(Box::pin(async move {
        let _guard = guard;
        let _ = report.send(());
        pending::<()>().await;
    }));
    within(&rt, started)
        .await
        .expect("the task did not start")
        .expect("the task's future was dropped before it started");
    task.abort();
    let end = within(&rt, task).await.expect("an aborted task did not end");
    assert!(end.is_aborted(), "a task aborted at an await ended {end:?}");
    assert_eq!(drops.load(Ordering::SeqCst), 1, "the handle answered before the task's future was dropped");
}

/// A panic ends the task `Panicked` with the panic's message, redacted: awaiting the handle
/// answers rather than re-raising it, polled again it answers the same message, shared rather than
/// copied, and the runtime runs the next task.
pub async fn panicked<R: Runtime>(rt: R) {
    let (open, gate) = oneshot::channel::<()>();
    let mut task = rt.spawn(Box::pin(async move {
        let _ = gate.await;
        panic!("{PANIC_MESSAGE}");
    }));
    let _ = open.send(());
    let end = within(&rt, &mut task).await.expect("a task that panics did not end");
    assert_panicked_with_the_message(&end);
    let again = within(&rt, &mut task).await.expect("a handle polled after its end did not answer");
    assert_panicked_with_the_message(&again);
    let (TaskEnd::Panicked(first), TaskEnd::Panicked(second)) = (&end, &again) else { unreachable!("both asserted `Panicked`") };
    assert!(Arc::ptr_eq(first, second), "polled again, the handle answered a copy of the message rather than the one it answered");
    let after = rt.spawn(Box::pin(async {}));
    let after = within(&rt, after).await;
    assert!(after.as_ref().is_some_and(TaskEnd::is_finished), "a task spawned after the panic ended {after:?}");
}

/// A panic raised while an aborted task's future is dropped leaves the end `Aborted` and is logged
/// at `warn`, carrying the panic's message. The record is read once the handle has answered, by
/// which time the future has been dropped.
pub async fn panic_while_aborted<R: Runtime>(rt: R) {
    let recorder = Recorder::installed();
    let (report, started) = oneshot::channel::<()>();
    let task = rt.spawn(Box::pin(async move {
        let _guard = PanicsWhenDropped;
        let _ = report.send(());
        pending::<()>().await;
    }));
    within(&rt, started)
        .await
        .expect("the task did not start")
        .expect("the task's future was dropped before it started");
    task.abort();
    let end = within(&rt, task).await.expect("an aborted task whose future panics while dropped did not end");
    assert!(end.is_aborted(), "a task whose future panicked while dropped after an abort ended {end:?}");
    let logged = recorder.warnings_containing(DROP_PANIC_MESSAGE);
    assert_eq!(logged.len(), 1, "the panic while the aborted future was dropped was logged at `warn` {} times", logged.len());
}

/// A handle polled again answers the end it answered, and `abort` after the end leaves it.
pub async fn end_kept<R: Runtime>(rt: R) {
    let mut task = rt.spawn(Box::pin(async {}));
    let first = within(&rt, &mut task).await.expect("a task that returns at once did not end");
    task.abort();
    let again = within(&rt, &mut task).await.expect("a handle polled after its end did not answer");
    assert!(first.is_finished() && again.is_finished(), "the end, then the end after an abort: {first:?}, {again:?}");
}

/// Panics in its drop, the way a task's state can when the task is aborted.
struct PanicsWhenDropped;

impl Drop for PanicsWhenDropped {
    fn drop(&mut self) {
        panic!("{DROP_PANIC_MESSAGE}");
    }
}
