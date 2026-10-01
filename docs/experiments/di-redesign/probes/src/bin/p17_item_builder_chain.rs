//! P17: one binding-handle type with a state parameter naming the item the last call wrote.
//! `.timeout` exists only after `.ready(..)` or an `on_*`; `.retries`/`.backoff` only after
//! `.ready(..)`; every other method is written once on `Handle<T, K>` and moves the handle to
//! the next state, so no sub-builder repeats the method list. The chain is the response's, plus
//! the module hook `m.on_init(..).timeout(..)`. Expected: compiles; prints each item's timeout.
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

pub struct Dep<T: ?Sized>(pub Arc<T>);
pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}
impl<F, Fut, A> Factory<(A,)> for F where F: Fn(A) -> Fut + Send + Sync + 'static, Fut: Future + Send {}

#[derive(Debug, Default)]
pub struct Item { pub name: &'static str, pub timeout: Option<Duration>, pub retries: u32 }

pub struct NoItem;
pub struct ReadyItem;
pub struct HookItem;
#[diagnostic::on_unimplemented(
    message = "`.timeout(..)` needs an item to apply to",
    note = "write it after `.ready(..)` or after an `on_*` hook"
)]
pub trait HasTimeout {}
impl HasTimeout for ReadyItem {}
impl HasTimeout for HookItem {}

pub struct Handle<T, K = NoItem> { pub items: Vec<Item>, _s: PhantomData<fn() -> (T, K)> }
impl<T, K> Handle<T, K> {
    fn with<K2>(mut self, name: &'static str) -> Handle<T, K2> {
        self.items.push(Item { name, ..Default::default() });
        Handle { items: self.items, _s: PhantomData }
    }
    fn last(&mut self) -> &mut Item { self.items.last_mut().expect("an item was written") }
    pub fn ready<Args, F: Factory<Args>>(self, _check: F) -> Handle<T, ReadyItem> { self.with("ready") }
    pub fn on_init<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem> { self.with("on_init") }
    pub fn on_destroy<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem> { self.with("on_destroy") }
    pub fn before_shutdown<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem> { self.with("before_shutdown") }
    pub fn on_shutdown<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem> { self.with("on_shutdown") }
    /// A method that writes no timed item leaves the chain with nothing for `.timeout` to apply to.
    pub fn also_as<U: ?Sized + 'static>(self) -> Handle<T, NoItem> { Handle { items: self.items, _s: PhantomData } }
}
impl<T, K: HasTimeout> Handle<T, K> {
    pub fn timeout(mut self, d: Duration) -> Self { self.last().timeout = Some(d); self }
}
impl<T> Handle<T, ReadyItem> {
    pub fn retries(mut self, n: u32) -> Self { self.last().retries = n; self }
    pub fn backoff(self, _d: Duration) -> Self { self }
}

pub struct ModuleDef { pub hooks: Vec<Item> }
pub struct ModuleHook<'m> { def: &'m mut ModuleDef }
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool> { Handle { items: Vec::new(), _s: PhantomData } }
    pub fn on_init<Args, F: Factory<Args>>(&mut self, _f: F) -> ModuleHook<'_> {
        self.hooks.push(Item { name: "module on_init", ..Default::default() });
        ModuleHook { def: self }
    }
}
impl<'m> ModuleHook<'m> {
    pub fn timeout(self, d: Duration) -> Self {
        let Self { def } = self;
        def.hooks.last_mut().expect("a hook was written").timeout = Some(d);
        ModuleHook { def }
    }
}

pub struct PgPool;
pub struct UserService;
fn secs(n: u64) -> Duration { Duration::from_secs(n) }

fn main() {
    let mut m = ModuleDef { hooks: Vec::new() };
    let h = m.singleton(|| async { PgPool })
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) }).retries(5).timeout(secs(10))   // the check's timeout
        .on_destroy(|_pool: Dep<PgPool>| async move {}).timeout(secs(3));                              // the hook's timeout
    let h2 = m.singleton(|| async { PgPool })
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) })
        .on_destroy(|_pool: Dep<PgPool>| async move {}).timeout(secs(1))
        .also_as::<dyn Send>();                                                                       // state back to NoItem
    m.on_init(|_u: Dep<UserService>| async move {}).timeout(secs(2));
    println!("binding: {:?}", h.items);
    println!("binding2: {:?}", h2.items);
    println!("module: {:?}", m.hooks);
}
