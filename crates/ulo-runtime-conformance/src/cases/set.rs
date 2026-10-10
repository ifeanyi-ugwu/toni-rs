//! A `TaskSet` over the runtime: `abort_all`, `join_all`, and what dropping it does.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use ulo::{Runtime, Spawn, TaskEnd, Timer};
use ulo_transport::TaskSet;

use super::{Dropped, SETTLE, TASKS, within, within_for};

/// `TASKS` tasks that each count their drop and wait forever, every one started when it returns.
async fn waiting_forever(rt: &Arc<dyn Runtime>, drops: &Arc<AtomicUsize>) -> TaskSet {
    let mut set = TaskSet::new(Arc::clone(rt) as Arc<dyn Spawn>);
    let (report, mut started) = mpsc::unbounded::<()>();
    for _ in 0..TASKS {
        let guard = Dropped(Arc::clone(drops));
        let report = report.clone();
        set.spawn(async move {
            let _guard = guard;
            let _ = report.unbounded_send(());
            pending::<()>().await;
        });
    }
    for _ in 0..TASKS {
        within(&**rt, started.next()).await.flatten().expect("a task in the set did not start");
    }
    set
}

/// `abort_all` ends every task `Aborted`, and the set is empty once each end is taken, every
/// future dropped by then.
pub async fn abort_all<R: Runtime>(rt: R) {
    let rt: Arc<dyn Runtime> = Arc::new(rt);
    let drops = Arc::new(AtomicUsize::new(0));
    let mut set = waiting_forever(&rt, &drops).await;
    assert_eq!(set.len(), TASKS, "the set's tasks before the abort");
    set.abort_all();
    let mut ends = Vec::new();
    while let Some(end) = within(&*rt, set.join_next()).await.expect("an aborted task in the set did not end") {
        ends.push(end);
    }
    assert!(
        ends.len() == TASKS && ends.iter().all(|end| matches!(end, TaskEnd::Aborted)),
        "every task after `abort_all`: {ends:?}"
    );
    assert_eq!(drops.load(Ordering::SeqCst), TASKS, "futures dropped once every end was taken");
    assert!(set.is_empty(), "the set after every end was taken");
}

/// `join_all` waits while any task runs and returns once the last has ended.
pub async fn join_all<R: Runtime>(rt: R) {
    let rt: Arc<dyn Runtime> = Arc::new(rt);
    let mut set = TaskSet::new(Arc::clone(&rt) as Arc<dyn Spawn>);
    let ended = Arc::new(AtomicUsize::new(0));
    let mut gates = Vec::new();
    for _ in 0..TASKS {
        let (open, gate) = oneshot::channel::<()>();
        gates.push(open);
        let ended = Arc::clone(&ended);
        set.spawn(async move {
            let _ = gate.await;
            ended.fetch_add(1, Ordering::SeqCst);
        });
    }
    let last = gates.pop().expect("one gate per task");
    for open in gates {
        let _ = open.send(());
    }
    let early = within_for(&*rt, SETTLE, set.join_all()).await;
    assert!(
        early.is_none(),
        "`join_all` returned with a task still running: {} of {TASKS} ended",
        ended.load(Ordering::SeqCst)
    );
    let _ = last.send(());
    within(&*rt, set.join_all()).await.expect("`join_all` did not return once every task had ended");
    assert_eq!(ended.load(Ordering::SeqCst), TASKS, "tasks ended when `join_all` returned");
    assert!(set.is_empty(), "the set after `join_all`");
}

/// Dropping the set aborts every task still in it.
pub async fn dropped<R: Runtime>(rt: R) {
    let rt: Arc<dyn Runtime> = Arc::new(rt);
    let drops = Arc::new(AtomicUsize::new(0));
    let set = waiting_forever(&rt, &drops).await;
    drop(set);
    let all = within(&*rt, async {
        while drops.load(Ordering::SeqCst) < TASKS {
            Timer::sleep(&*rt, Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(all.is_some(), "{} of {TASKS} tasks were dropped after their set was", drops.load(Ordering::SeqCst));
}
