//! P14: `execute` as an inherent `async fn` bounded `F: AsyncFnOnce(&Execution) -> R`, called
//! from a concrete site as `app.execute(async |exec| ..)`. The returned future is `Send` through
//! auto-trait leakage whenever the caller's closure future is: `F` is concrete at the call, so
//! `F::CallOnceFuture` is known. Expected: compiles; the trait-method twin is in p14b.
use std::future::Future;
use std::sync::Arc;

pub struct Execution { inner: Arc<Inner> }
struct Inner { name: &'static str }
impl Execution {
    pub async fn get(&self) -> Result<&'static str, ()> { Ok(self.inner.name) }
    pub fn handle(&self) -> ExecutionRef { ExecutionRef(self.inner.clone()) }
}
#[derive(Clone)]
pub struct ExecutionRef(Arc<Inner>);
impl ExecutionRef { pub fn name(&self) -> &'static str { self.0.name } }

pub struct App;
impl App {
    pub async fn execute<F, R>(&self, f: F) -> R
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        let exec = Execution { inner: Arc::new(Inner { name: "borrowed" }) };
        let out = f(&exec).await;
        drop(exec);
        out
    }
}

fn assert_send<T: Send>(_: &T) {}

fn main() {
    let app = App;
    let fut = app.execute(async |exec| {
        let name = exec.get().await;
        let h = exec.handle();
        (name, h.name())
    });
    assert_send(&fut);
    let out = block_on(fut);
    println!("{out:?}");
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
