//! P17c: `.retries(..)` after `.on_destroy(..)`: the handle is in `HookItem` and `retries` is
//! defined on `Handle<T, ReadyItem>` only. Expected: fails, E0599 no method `retries`.
use std::future::Future;
use std::marker::PhantomData;

pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}

pub struct NoItem;
pub struct ReadyItem;
pub struct HookItem;

pub struct Handle<T, K = NoItem> { _s: PhantomData<fn() -> (T, K)> }
impl<T, K> Handle<T, K> {
    pub fn on_destroy<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem> { Handle { _s: PhantomData } }
}
impl<T> Handle<T, ReadyItem> {
    pub fn retries(self, _n: u32) -> Self { self }
}
pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool> { Handle { _s: PhantomData } }
}
pub struct PgPool;

fn main() {
    let mut m = ModuleDef;
    let _h = m.singleton(|| async { PgPool }).on_destroy(|| async {}).retries(5);
}
