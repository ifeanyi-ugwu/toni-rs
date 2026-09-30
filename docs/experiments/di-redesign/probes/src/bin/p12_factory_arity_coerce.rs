//! P12: one `singleton(f)` over closures of arity 0, 1 and 2 whose parameters are sites, with
//! the output key taken from the future's output; the two-closure
//! `contribute::<dyn HI>().singleton(factory, |a| a)` where `T` is inferred from the first
//! closure before the second is checked; and a builder chain `.ready(..).retries(..).timeout(..)
//! .on_destroy(..)`. Expected: compiles; sites are described from the parameter types.
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait Site: Sized + Send + 'static { fn describe(d: &mut Vec<&'static str>); }
pub struct Dep<T: ?Sized>(pub Arc<T>);
impl<T: ?Sized + Send + Sync + 'static> Site for Dep<T> { fn describe(d: &mut Vec<&'static str>) { d.push(std::any::type_name::<T>()); } }

pub trait Factory<Args>: Send + Sync + 'static {
    type Output: Send + Sync + 'static;
    fn describe(d: &mut Vec<&'static str>);
}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send, Fut::Output: Send + Sync + 'static {
    type Output = Fut::Output;
    fn describe(_d: &mut Vec<&'static str>) {}
}
impl<F, Fut, A> Factory<(A,)> for F where A: Site, F: Fn(A) -> Fut + Send + Sync + 'static, Fut: Future + Send, Fut::Output: Send + Sync + 'static {
    type Output = Fut::Output;
    fn describe(d: &mut Vec<&'static str>) { A::describe(d); }
}
impl<F, Fut, A, B> Factory<(A, B)> for F where A: Site, B: Site, F: Fn(A, B) -> Fut + Send + Sync + 'static, Fut: Future + Send, Fut::Output: Send + Sync + 'static {
    type Output = Fut::Output;
    fn describe(d: &mut Vec<&'static str>) { A::describe(d); B::describe(d); }
}

pub struct SingletonHandle<T> { pub sites: Vec<&'static str>, pub hooks: Vec<&'static str>, _t: PhantomData<fn() -> T> }
impl<T> SingletonHandle<T> {
    pub fn ready<Args, F: Factory<Args>>(mut self, _check: F) -> Self { self.hooks.push("ready"); self }
    pub fn retries(mut self, _n: u32) -> Self { self.hooks.push("retries"); self }
    pub fn timeout(mut self, _d: std::time::Duration) -> Self { self.hooks.push("timeout"); self }
    pub fn on_destroy<Args, F: Factory<Args>>(mut self, _f: F) -> Self { self.hooks.push("on_destroy"); self }
}
pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> SingletonHandle<F::Output> {
        let mut sites = Vec::new(); F::describe(&mut sites);
        SingletonHandle { sites, hooks: Vec::new(), _t: PhantomData }
    }
    pub fn contribute<U: ?Sized + Send + Sync + 'static>(&mut self) -> Contribute<U> { Contribute(PhantomData) }
}
pub struct Contribute<U: ?Sized>(PhantomData<fn() -> U>);
impl<U: ?Sized + Send + Sync + 'static> Contribute<U> {
    pub fn singleton<Args, F, C>(self, _f: F, _c: C) -> Vec<&'static str>
    where F: Factory<Args>, C: Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static {
        let mut d = Vec::new(); F::describe(&mut d); d
    }
}

pub struct DbConfig { pub url: String } pub struct PgPool; pub struct Clock;
pub trait HealthIndicator: Send + Sync {} pub struct PgHealth(Dep<PgPool>); impl HealthIndicator for PgHealth {}
fn main() {
    let mut m = ModuleDef;
    let h0 = m.singleton(|| async { PgPool });
    let h1 = m.singleton(|cfg: Dep<DbConfig>| async move { let _ = &cfg.0.url; PgPool });
    let h2 = m.singleton(|_p: Dep<PgPool>, _c: Dep<Clock>| async move { 7u32 });
    let chained = m.singleton(|cfg: Dep<DbConfig>| async move { let _ = cfg; PgPool })
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) })
        .retries(5).timeout(std::time::Duration::from_secs(10))
        .on_destroy(|_pool: Dep<PgPool>| async move {});
    let contributed = m.contribute::<dyn HealthIndicator>().singleton(|pool: Dep<PgPool>| async move { PgHealth(pool) }, |a| a);
    println!("h0 {:?} h1 {:?} h2 {:?} chain {:?} contributed {:?}", h0.sites, h1.sites, h2.sites, chained.hooks, contributed);
}
