//! P10: `app.execute(opts, |exec| async move { exec.get().await })`. A plain closure returning
//! an `async move` block cannot borrow its `&Execution` argument into the future; an `async`
//! closure under an `AsyncFnOnce` bound (stable 1.85) can, and an owned handle needs neither.
//! Expected: compiles; the failing shape is in p10b.
use std::future::Future;
use std::sync::Arc;
pub struct Execution { pub name: &'static str }
impl Execution { pub async fn get(&self) -> Result<&'static str, ()> { Ok(self.name) } }
#[derive(Clone)] pub struct ExecutionRef(Arc<Execution>);
impl ExecutionRef { pub async fn get(&self) -> Result<&'static str, ()> { self.0.get().await } }

pub async fn execute_async_closure<F, R>(f: F) -> R where F: AsyncFnOnce(&Execution) -> R {
    let exec = Execution { name: "borrowed" };
    f(&exec).await
}
pub async fn execute_owned<F, Fut, R>(f: F) -> R where F: FnOnce(ExecutionRef) -> Fut, Fut: Future<Output = R> {
    let exec = ExecutionRef(Arc::new(Execution { name: "owned" }));
    let out = f(exec.clone()).await;
    drop(exec);
    out
}
fn main() {
    let a = block_on(execute_async_closure(async |exec| { exec.get().await }));
    let b = block_on(execute_owned(|exec| async move { exec.get().await }));
    println!("{a:?} {b:?}");
}
fn block_on<F: Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop { if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; } }
}
