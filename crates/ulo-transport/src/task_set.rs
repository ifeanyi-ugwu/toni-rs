//! A runtime-free set of spawned tasks, for the work a server spawns per call or per connection:
//! spawned through the app's `Spawn`, aborted together at close, waited for together in the
//! drain.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use futures_util::stream::FuturesUnordered;
use ulo::{Spawn, TaskEnd, TaskHandle};

/// Tasks spawned through one runtime and owned together. A task runs from [`spawn`](Self::spawn)
/// on whether the set is polled or not; polling it only collects the ends, and a task stays in
/// [`len`](Self::len) until [`join_next`](Self::join_next) or [`join_all`](Self::join_all) has taken
/// its end.
///
/// Dropping the set aborts every task still in it, unlike dropping a lone [`TaskHandle`], which
/// detaches: the set's owner going away is the signal its tasks have no one left to serve.
pub struct TaskSet {
    runtime: Arc<dyn Spawn>,
    tasks: FuturesUnordered<TaskHandle>,
}

impl TaskSet {
    /// An empty set spawning through `runtime`; an `Arc<dyn Runtime>` coerces to the argument.
    pub fn new(runtime: Arc<dyn Spawn>) -> TaskSet {
        TaskSet { runtime, tasks: FuturesUnordered::new() }
    }

    pub fn spawn<F>(&mut self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.push(self.runtime.spawn(Box::pin(fut)));
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Aborts every task in the set. Each ends `Aborted`, unless its future returned or panicked
    /// first, and is taken out by the next `join_next` or `join_all`, like any other end.
    pub fn abort_all(&self) {
        for task in self.tasks.iter() {
            task.abort();
        }
    }

    /// The end of the next task to end, taken out of the set; `None` when the set is empty.
    /// Dropped before it answers, it takes nothing out, so it can stand as one branch of a
    /// `select` that runs again.
    pub async fn join_next(&mut self) -> Option<TaskEnd> {
        poll_fn(|cx| Pin::new(&mut self.tasks).poll_next(cx)).await
    }

    /// Resolves once every task in the set has ended, its future dropped, leaving the set empty.
    /// After [`abort_all`](Self::abort_all), that is once each abort has taken effect.
    pub async fn join_all(&mut self) {
        while self.join_next().await.is_some() {}
    }
}

impl Drop for TaskSet {
    fn drop(&mut self) {
        self.abort_all();
    }
}
