//! A lone `TaskHandle`: the end it answers after a return, an abort and a panic, and what dropping
//! it does.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::channel::oneshot;
use ulo::{Runtime, TaskEnd};

use super::{Dropped, within};

/// A future that returns ends the task `Finished`, after it ran.
pub async fn finished<R: Runtime>(rt: R) {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let task = rt.spawn(Box::pin(async move { flag.store(true, Ordering::SeqCst) }));
    let end = within(&rt, task).await.expect("a task that returns at once did not end");
    assert_eq!(end, TaskEnd::Finished, "a task whose future returned");
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
    assert_eq!(end, TaskEnd::Aborted, "a task aborted at an await");
    assert_eq!(drops.load(Ordering::SeqCst), 1, "the handle answered before the task's future was dropped");
}

/// A panic ends the task `Panicked`: awaiting the handle answers rather than re-raising it, and the
/// runtime runs the next task.
pub async fn panicked<R: Runtime>(rt: R) {
    let (open, gate) = oneshot::channel::<()>();
    let task = rt.spawn(Box::pin(async move {
        let _ = gate.await;
        panic!("ulo-runtime-conformance: a deliberate panic inside a task");
    }));
    let _ = open.send(());
    let end = within(&rt, task).await.expect("a task that panics did not end");
    assert_eq!(end, TaskEnd::Panicked, "a task whose future panicked");
    let after = rt.spawn(Box::pin(async {}));
    assert_eq!(within(&rt, after).await, Some(TaskEnd::Finished), "a task spawned after the panic");
}

/// A handle polled again answers the end it answered, and `abort` after the end leaves it.
pub async fn end_kept<R: Runtime>(rt: R) {
    let mut task = rt.spawn(Box::pin(async {}));
    let first = within(&rt, &mut task).await.expect("a task that returns at once did not end");
    task.abort();
    let again = within(&rt, &mut task).await.expect("a handle polled after its end did not answer");
    assert_eq!((first, again), (TaskEnd::Finished, TaskEnd::Finished), "the end, then the end after an abort");
}
