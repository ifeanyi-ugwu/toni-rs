//! P01: `|a| a` as an unsizing coercion at a closure's return when the expected type comes
//! from `F: FnOnce(Arc<T>) -> Arc<U>` with `U` a `dyn Trait` named by turbofish.
//! Also: the stored erased coercion end to end, and the role twin (`dyn ErasedGuard<Http>`).
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub trait Cache: Send + Sync { fn name(&self) -> &'static str; }
pub struct RedisCache;
impl Cache for RedisCache { fn name(&self) -> &'static str { "redis" } }

type Erased = Arc<dyn Any + Send + Sync>;
type Coercion = Box<dyn Fn(Erased) -> Box<dyn Any + Send + Sync> + Send + Sync>;

pub struct Handle<T> { coercions: Vec<Coercion>, _t: std::marker::PhantomData<fn() -> T> }

impl<T: Send + Sync + 'static> Handle<T> {
    pub fn new() -> Self { Self { coercions: Vec::new(), _t: std::marker::PhantomData } }
    pub fn also_as<U, F>(mut self, f: F) -> Self
    where
        U: ?Sized + Send + Sync + 'static,
        F: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    {
        self.coercions.push(Box::new(move |slot: Erased| {
            let concrete: Arc<T> = slot.downcast::<T>().expect("slot holds T");
            let widened: Arc<U> = f(concrete);
            Box::new(widened)
        }));
        self
    }
}

// The role twin: a non-dyn-compatible trait with an RPITIT method, erased by a blanket impl.
pub trait Transport: 'static { type Cx: Send; }
pub struct Http;
impl Transport for Http { type Cx = String; }
pub trait Guard<T: Transport>: Send + Sync + 'static {
    fn can_activate(&self, cx: &mut T::Cx) -> impl Future<Output = bool> + Send;
}
pub trait ErasedGuard<T: Transport>: Send + Sync + 'static {
    fn can_activate<'a>(&'a self, cx: &'a mut T::Cx) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}
impl<T: Transport, G: Guard<T>> ErasedGuard<T> for G {
    fn can_activate<'a>(&'a self, cx: &'a mut T::Cx) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(Guard::can_activate(self, cx))
    }
}
pub type AnyGuard<T> = dyn ErasedGuard<T>;
pub struct AuthGuard;
impl Guard<Http> for AuthGuard {
    async fn can_activate(&self, cx: &mut String) -> bool { cx.push('!'); true }
}

fn main() {
    let h = Handle::<RedisCache>::new().also_as::<dyn Cache, _>(|a| a);
    let slot: Erased = Arc::new(RedisCache);
    let boxed = (h.coercions[0])(slot);
    let cache: Arc<dyn Cache> = *boxed.downcast::<Arc<dyn Cache>>().expect("Arc<dyn Cache>");
    println!("also_as: {}", cache.name());

    let g = Handle::<AuthGuard>::new().also_as::<AnyGuard<Http>, _>(|a| a);
    let boxed = (g.coercions[0])(Arc::new(AuthGuard));
    let guard: Arc<AnyGuard<Http>> = *boxed.downcast::<Arc<AnyGuard<Http>>>().expect("Arc<AnyGuard<Http>>");
    let mut cx = String::from("cx");
    let fut = guard.can_activate(&mut cx);
    let admitted = pollster_lite(fut);
    println!("role twin: admitted={admitted} cx={cx}");
}

fn pollster_lite<F: Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop { if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; } }
}
