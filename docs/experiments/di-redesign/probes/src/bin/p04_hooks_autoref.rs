//! P04: `Construct::hooks` filled by autoref probing inside a generated impl with concrete
//! `Self`, storing `for<'a> Fn(&'a T) -> BoxFuture<'a, _>` closures. Expected: compiles; the
//! type with the impl registers one hook, the type without registers none.
use std::future::Future;
use std::pin::Pin;
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Scope: 'static {} pub struct Auto; impl Scope for Auto {}
pub trait HookCapable: Scope {} impl HookCapable for Auto {}
pub trait Construct: Sized + Send + Sync + 'static { type Scope: Scope; fn hooks(_h: &mut Hooks<Self>) {} }
pub trait OnModuleInit: Construct<Scope: HookCapable> {
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub struct Hooks<T> { pub init: Vec<Box<dyn for<'a> Fn(&'a T) -> BoxFuture<'a, Result<(), BoxError>> + Send + Sync>> }
impl<T> Hooks<T> {
    pub fn on_init<F>(&mut self, f: F) where F: for<'a> Fn(&'a T) -> BoxFuture<'a, Result<(), BoxError>> + Send + Sync + 'static {
        self.init.push(Box::new(f));
    }
}

// The probe pair the macro emits: `&HookProbe<T>` reaches the registering arm only when
// `T: OnModuleInit`; otherwise method resolution autorefs to the no-op arm.
pub struct HookProbe<T>(std::marker::PhantomData<fn() -> T>);
pub trait RegisterInit<T> { fn register_init(&self, h: &mut Hooks<T>); }
pub trait NoInit<T> { fn register_init(&self, _h: &mut Hooks<T>) {} }
impl<T: OnModuleInit> RegisterInit<T> for HookProbe<T> {
    fn register_init(&self, h: &mut Hooks<T>) { h.on_init(|s| Box::pin(s.on_module_init())); }
}
impl<T> NoInit<T> for &HookProbe<T> {}

pub struct Cache;
impl Construct for Cache {
    type Scope = Auto;
    fn hooks(h: &mut Hooks<Self>) { (&HookProbe::<Self>(Default::default())).register_init(h); }
}
impl OnModuleInit for Cache {
    async fn on_module_init(&self) -> Result<(), BoxError> { println!("cache init ran"); Ok(()) }
}
pub struct Plain;
impl Construct for Plain {
    type Scope = Auto;
    fn hooks(h: &mut Hooks<Self>) { (&HookProbe::<Self>(Default::default())).register_init(h); }
}

fn main() {
    let mut hc = Hooks::<Cache> { init: Vec::new() };
    Cache::hooks(&mut hc);
    let mut hp = Hooks::<Plain> { init: Vec::new() };
    Plain::hooks(&mut hp);
    println!("cache hooks={} plain hooks={}", hc.init.len(), hp.init.len());
    let c = Cache;
    let _ = fable_redesign_poll(hc.init[0](&c));
}
fn fable_redesign_poll<F: Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop { if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; } }
}
