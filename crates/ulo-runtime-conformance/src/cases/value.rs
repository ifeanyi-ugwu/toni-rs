//! `spawn_with`: the value a task answers, and an abort or a panic in its place.

use std::future::pending;

use futures::channel::oneshot;
use ulo::{Runtime, TaskEnd, spawn_with};

use super::{PANIC_MESSAGE, assert_panicked_with_the_message, within};

pub async fn answered<R: Runtime>(rt: R) {
    let task = spawn_with(&rt, async { 7_u32 });
    let answer = within(&rt, task).await;
    assert!(matches!(answer, Some(Ok(7))), "the value the task's future returned: {answer:?}");
}

/// Polling after the value was taken answers `Err(TaskEnd::Finished)`.
pub async fn taken<R: Runtime>(rt: R) {
    let mut task = spawn_with(&rt, async { 7_u32 });
    let first = within(&rt, &mut task).await;
    assert!(matches!(first, Some(Ok(7))), "the value the task's future returned: {first:?}");
    let again = within(&rt, &mut task).await;
    assert!(matches!(again, Some(Err(TaskEnd::Finished))), "the handle polled after its value was taken: {again:?}");
}

pub async fn aborted<R: Runtime>(rt: R) {
    let (report, started) = oneshot::channel::<()>();
    let task = spawn_with(&rt, async move {
        let _ = report.send(());
        pending::<u32>().await
    });
    within(&rt, started)
        .await
        .expect("the task did not start")
        .expect("the task's future was dropped before it started");
    task.abort();
    let answer = within(&rt, task).await;
    assert!(matches!(answer, Some(Err(TaskEnd::Aborted))), "a task aborted at an await: {answer:?}");
}

pub async fn panicked<R: Runtime>(rt: R) {
    let task = spawn_with(&rt, async {
        if true {
            panic!("{PANIC_MESSAGE}");
        }
        7_u32
    });
    match within(&rt, task).await {
        Some(Err(end)) => assert_panicked_with_the_message(&end),
        other => panic!("a task whose future panicked answered {other:?}"),
    }
}
