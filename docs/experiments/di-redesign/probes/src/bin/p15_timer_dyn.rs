//! P15: a `Timer` trait reachable as `Dep<dyn Timer>`. `sleep` returns a boxed future, which is
//! what makes the trait dyn-compatible; the core's `timeout` combinator is built from `sleep`
//! alone with no runtime; a service holding `Dep<dyn Timer>` has a `Send` hook future; and the
//! `dyn Timer` key is compared with `dyn Timer + Send + Sync`. Expected: compiles and runs.
use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The one method the core needs. `Send + Sync` supertraits make `dyn Timer` satisfy `Dep`'s bound.
pub trait Timer: Send + Sync + 'static {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
}

pub struct Dep<T: ?Sized>(Arc<T>);
impl<T: ?Sized + Send + Sync + 'static> Dep<T> { pub fn from_arc(a: Arc<T>) -> Self { Dep(a) } }
impl<T: ?Sized> std::ops::Deref for Dep<T> { type Target = T; fn deref(&self) -> &T { &self.0 } }

#[derive(Debug)]
pub struct TimedOut;
/// Built from `sleep` only: `sleep_until` is `sleep(deadline - Instant::now())`, and this select needs no runtime.
pub async fn timeout<F: Future>(timer: &dyn Timer, d: Duration, fut: F) -> Result<F::Output, TimedOut> {
    let mut fut = Box::pin(fut);
    let mut sleep = timer.sleep(d);
    std::future::poll_fn(|cx| {
        if let Poll::Ready(v) = fut.as_mut().poll(cx) { return Poll::Ready(Ok(v)); }
        if let Poll::Ready(()) = sleep.as_mut().poll(cx) { return Poll::Ready(Err(TimedOut)); }
        Poll::Pending
    })
    .await
}

// Two runtime adapters: one whose sleep never ends, one whose sleep ends at once.
pub struct NeverTimer;
impl Timer for NeverTimer { fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> { Box::pin(std::future::pending()) } }
pub struct InstantTimer;
impl Timer for InstantTimer { fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> { Box::pin(std::future::ready(())) } }

// The user's example: a service that sleeps in its before-shutdown hook through the binding.
pub struct Discovery { timer: Dep<dyn Timer> }
impl Discovery {
    pub async fn before_application_shutdown(&self) { self.timer.sleep(Duration::from_secs(5)).await }
}
fn assert_send<F: Future + Send>(_: F) {}

fn main() {
    let bound: Arc<dyn Timer> = Arc::new(InstantTimer);           // what the builder's `.timer(..)` binds
    let dep = Dep::<dyn Timer>::from_arc(bound);
    let discovery = Discovery { timer: dep };
    assert_send(discovery.before_application_shutdown());
    poll(discovery.before_application_shutdown());

    let ok = poll(timeout(&NeverTimer, Duration::from_secs(1), std::future::ready(5u8)));
    let late = poll(timeout(&InstantTimer, Duration::from_secs(1), std::future::pending::<u8>()));
    println!("timeout: finished={ok:?} expired={late:?}");
    println!(
        "TypeId dyn Timer == dyn Timer + Send + Sync: {}",
        TypeId::of::<dyn Timer>() == TypeId::of::<dyn Timer + Send + Sync>()
    );
}

fn poll<F: Future>(fut: F) -> F::Output {
    use std::task::{Context, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop { if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; } }
}
