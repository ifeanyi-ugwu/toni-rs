//! A `Tokio` spawns, sleeps and runs a future on the runtime it holds from a thread where no
//! runtime is current, as a client's drop runs once `block_on` has returned or on a plain
//! `std::thread`, and as a tokio-based link is called from another executor.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Waker};
use std::thread;
use std::time::Duration;

use futures_executor::block_on;
use tokio::runtime::{Builder, Handle, Runtime as TokioRuntime};
use ulo::{Spawn, Timer};
use ulo_tokio::{Stopped, Tokio};

const PATIENCE: Duration = Duration::from_secs(5);

fn held_runtime() -> TokioRuntime {
    Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("a tokio runtime for the test")
}

#[test]
fn spawns_from_a_thread_outside_any_runtime() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    let (ran, finished) = mpsc::channel();
    let task = thread::spawn(move || {
        assert!(Handle::try_current().is_err(), "the plain thread has a tokio runtime current");
        held.spawn(Box::pin(async move {
            let _ = ran.send(());
        }))
    })
    .join()
    .expect("spawning from a plain thread panicked");
    finished.recv_timeout(PATIENCE).expect("the task spawned from a plain thread did not run");
    let end = runtime.block_on(task);
    assert!(end.is_finished(), "the task spawned from a plain thread ended {end:?}");
}

#[test]
fn sleeps_from_a_thread_outside_any_runtime() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    let sleep = thread::spawn(move || {
        assert!(Handle::try_current().is_err(), "the plain thread has a tokio runtime current");
        held.sleep(Duration::from_millis(10))
    })
    .join()
    .expect("creating a sleep on a plain thread panicked");
    runtime.block_on(async {
        tokio::time::timeout(PATIENCE, sleep).await.expect("a sleep created on a plain thread did not end");
    });
}

#[test]
fn run_answers_on_a_thread_outside_any_runtime() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    let answer = thread::spawn(move || {
        assert!(Handle::try_current().is_err(), "the plain thread has a tokio runtime current");
        // `tokio::time::sleep` panics outside a runtime, so the answer shows where the future ran.
        block_on(held.run(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            7
        }))
    })
    .join()
    .expect("running a future from a plain thread panicked");
    assert_eq!(answer, Ok(7));
}

#[test]
fn dropping_the_answer_aborts_the_task() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    let (started, running) = mpsc::channel();
    let (dropped, gone) = mpsc::channel();
    thread::spawn(move || {
        let guard = Guard(dropped);
        let mut answer = pin!(held.run(async move {
            let _guard = guard;
            let _ = started.send(());
            std::future::pending::<()>().await;
        }));
        // The first poll spawns the task.
        assert!(answer.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        running.recv_timeout(PATIENCE).expect("the task did not start");
    })
    .join()
    .expect("the plain thread panicked");
    gone.recv_timeout(PATIENCE).expect("the task's future was not dropped after its answer was");
}

#[test]
fn a_panic_in_the_task_resumes_where_the_answer_is_awaited() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    let caught = catch_unwind(AssertUnwindSafe(|| block_on(held.run(async { panic!("a deliberate panic inside the task") }))));
    let payload = caught.expect_err("the task's panic did not reach the awaiter");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"a deliberate panic inside the task"));
}

#[test]
fn a_runtime_that_has_shut_down_answers_stopped() {
    let runtime = held_runtime();
    let held = Tokio::from_handle(runtime.handle().clone());
    drop(runtime);
    assert_eq!(block_on(held.run(async { 7 })), Err(Stopped));
}

/// Reports its drop.
struct Guard(mpsc::Sender<()>);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
