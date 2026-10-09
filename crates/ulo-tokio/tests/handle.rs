//! A `Tokio` spawns and sleeps on the runtime it holds from a thread where no runtime is current,
//! as a client's drop runs once `block_on` has returned or on a plain `std::thread`.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tokio::runtime::{Builder, Handle, Runtime as TokioRuntime};
use ulo::{Spawn, TaskEnd, Timer};
use ulo_tokio::Tokio;

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
    assert_eq!(runtime.block_on(task), TaskEnd::Finished, "the task spawned from a plain thread");
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
