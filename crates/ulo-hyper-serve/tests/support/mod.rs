//! What the tests share: a smol executor run by the test's thread and one more, the app's runtime
//! on it, a counting runtime, and a bounded wait.

#![allow(dead_code)]

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};

use futures::FutureExt;
use ulo::{BoxFuture, Spawn, TaskHandle, Timer};
use ulo_smol::{Executor, Smol};

/// How long any one step may take.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// Runs `test` with a `Smol` runtime on an executor of its own, run by this thread and one more.
pub fn on_smol<F, Fut>(test: F) -> Fut::Output
where
    F: FnOnce(Smol) -> Fut,
    Fut: Future,
{
    let executor = Arc::new(Executor::new());
    let (stop, stopped) = futures::channel::oneshot::channel::<()>();
    let worker = {
        let executor = Arc::clone(&executor);
        std::thread::spawn(move || async_io::block_on(executor.run(stopped.map(|_| ()))))
    };
    let output = async_io::block_on(executor.run(test(Smol::new(Arc::clone(&executor)))));
    drop(stop);
    worker.join().expect("the executor's second thread panicked");
    output
}

/// `fut`'s output, or a failure naming `what` once [`PATIENCE`] has passed.
pub async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut expired = pin!(async_io::Timer::after(PATIENCE));
    poll_fn(|cx| {
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(output);
        }
        if expired.as_mut().poll(cx).is_ready() {
            panic!("{what} did not happen within {PATIENCE:?}");
        }
        Poll::Pending
    })
    .await
}

/// A runtime counting each task it is handed.
#[derive(Clone)]
pub struct Counting {
    pub smol: Smol,
    pub spawned: Arc<AtomicUsize>,
}

impl Counting {
    pub fn new(smol: Smol) -> Counting {
        Counting { smol, spawned: Arc::default() }
    }

    pub fn spawned(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

impl Timer for Counting {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.smol.sleep(d)
    }

    fn now(&self) -> Instant {
        self.smol.now()
    }
}

impl Spawn for Counting {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.smol.spawn(fut)
    }
}
