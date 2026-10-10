//! The runtime conformance suite over `ulo_smol::Smol`, each scenario on an executor of its own,
//! run by the test's thread and three more, so a task may be polled on another thread than the
//! one aborting or awaiting it.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

use futures::FutureExt;
use futures::channel::oneshot;
use ulo_runtime_conformance::{Harness, runtime_suite};
use ulo_smol::{Executor, Smol};

/// The threads running the scenario's executor beside the test's own.
const WORKERS: usize = 3;

thread_local! {
    /// The executor of the scenario this thread runs, for `runtime()`, which `block_on`'s future
    /// calls on this thread.
    static EXECUTOR: RefCell<Option<Arc<Executor<'static>>>> = const { RefCell::new(None) };
}

struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;

    fn runtime() -> Smol {
        let executor = EXECUTOR.with(|current| current.borrow().clone()).expect("`runtime()` is called inside `block_on`");
        Smol::new(executor)
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let executor = Arc::new(Executor::new());
        let (stop, stopped) = oneshot::channel::<()>();
        let stopped = stopped.shared();
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                let executor = Arc::clone(&executor);
                let stopped = stopped.clone();
                std::thread::spawn(move || async_io::block_on(executor.run(stopped)))
            })
            .collect();
        EXECUTOR.with(|current| *current.borrow_mut() = Some(Arc::clone(&executor)));
        let output = async_io::block_on(executor.run(fut));
        EXECUTOR.with(|current| *current.borrow_mut() = None);
        drop(stop);
        for worker in workers {
            let _ = worker.join().expect("a worker running the scenario's executor panicked");
        }
        output
    }
}

runtime_suite!(OnSmol);
