//! P16: `const TIMEOUT: Option<Duration>` with a default on a hook trait that also declares an
//! RPITIT method; two hook traits on one type each carry their own `TIMEOUT`; the generated
//! `Hooks<T>` registration reads the const through the autoref arm and stores it beside the
//! closure. Expected: compiles on 1.88 and 1.98; prints `Some(10s)` for the override, `None`
//! for the default, and no hooks for the type without impls.
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Construct: Sized + Send + Sync + 'static { fn hooks(_h: &mut Hooks<Self>) {} }

pub trait OnModuleInit: Construct {
    /// `None` means the app's `hook_timeout`.
    const TIMEOUT: Option<Duration> = None;
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnModuleDestroy: Construct {
    const TIMEOUT: Option<Duration> = None;
    fn on_module_destroy(&self) -> impl Future<Output = ()> + Send;
}

pub struct Hook<T, O> {
    pub timeout: Option<Duration>,
    pub run: Box<dyn for<'a> Fn(&'a T) -> BoxFuture<'a, O> + Send + Sync>,
}
pub struct Hooks<T> { pub init: Option<Hook<T, Result<(), BoxError>>>, pub destroy: Option<Hook<T, ()>> }

pub struct HookProbe<T>(std::marker::PhantomData<fn() -> T>);
pub trait RegisterInit<T> { fn register_init(&self, h: &mut Hooks<T>); }
pub trait NoInit<T> { fn register_init(&self, _h: &mut Hooks<T>) {} }
impl<T: OnModuleInit> RegisterInit<T> for HookProbe<T> {
    fn register_init(&self, h: &mut Hooks<T>) {
        h.init = Some(Hook { timeout: <T as OnModuleInit>::TIMEOUT, run: Box::new(|s| Box::pin(s.on_module_init())) });
    }
}
impl<T> NoInit<T> for &HookProbe<T> {}
pub trait RegisterDestroy<T> { fn register_destroy(&self, h: &mut Hooks<T>); }
pub trait NoDestroy<T> { fn register_destroy(&self, _h: &mut Hooks<T>) {} }
impl<T: OnModuleDestroy> RegisterDestroy<T> for HookProbe<T> {
    fn register_destroy(&self, h: &mut Hooks<T>) {
        h.destroy = Some(Hook { timeout: <T as OnModuleDestroy>::TIMEOUT, run: Box::new(|s| Box::pin(s.on_module_destroy())) });
    }
}
impl<T> NoDestroy<T> for &HookProbe<T> {}

pub struct Cache;
impl Construct for Cache {
    fn hooks(h: &mut Hooks<Self>) {
        let p = HookProbe::<Self>(Default::default());
        (&p).register_init(h);
        (&p).register_destroy(h);
    }
}
impl OnModuleInit for Cache {
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(10));
    async fn on_module_init(&self) -> Result<(), BoxError> { Ok(()) }
}
impl OnModuleDestroy for Cache {
    async fn on_module_destroy(&self) {}
}
pub struct Plain;
impl Construct for Plain {
    fn hooks(h: &mut Hooks<Self>) {
        let p = HookProbe::<Self>(Default::default());
        (&p).register_init(h);
        (&p).register_destroy(h);
    }
}

fn main() {
    let mut hc = Hooks::<Cache> { init: None, destroy: None };
    Cache::hooks(&mut hc);
    let mut hp = Hooks::<Plain> { init: None, destroy: None };
    Plain::hooks(&mut hp);
    println!(
        "cache init={:?} destroy={:?}",
        hc.init.as_ref().map(|h| h.timeout),
        hc.destroy.as_ref().map(|h| h.timeout)
    );
    println!("plain init={} destroy={}", hp.init.is_some(), hp.destroy.is_some());
}
