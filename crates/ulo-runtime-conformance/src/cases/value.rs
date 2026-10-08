//! `spawn_with`: the value a task answers, and an abort or a panic in its place.

use std::future::pending;

use futures::channel::oneshot;
use ulo::{Runtime, TaskEnd, spawn_with};

use super::within;

pub async fn answered<R: Runtime>(rt: R) {
    let task = spawn_with(&rt, async { 7_u32 });
    assert_eq!(within(&rt, task).await, Some(Ok(7)), "the value the task's future returned");
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
    assert_eq!(within(&rt, task).await, Some(Err(TaskEnd::Aborted)), "a task aborted at an await");
}

pub async fn panicked<R: Runtime>(rt: R) {
    let task = spawn_with(&rt, async {
        if true {
            panic!("ulo-runtime-conformance: a deliberate panic inside a task");
        }
        7_u32
    });
    assert_eq!(within(&rt, task).await, Some(Err(TaskEnd::Panicked)), "a task whose future panicked");
}
