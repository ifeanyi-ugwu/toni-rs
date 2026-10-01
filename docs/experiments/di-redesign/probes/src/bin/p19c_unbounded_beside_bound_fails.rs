//! P19c: `.unbounded()` beside an explicit bound on a readiness check, in both orders:
//! `.attempt_timeout(..).unbounded()` and `.unbounded().attempt_timeout(..)`. Expected: fails, two
//! E0599, on `unbounded` and on `attempt_timeout`.
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}

pub struct Open;
pub struct Set;
pub struct Construction<B>(PhantomData<B>);
pub struct ReadyItem<W, A>(PhantomData<(W, A)>);

pub trait Unbounded<C> { type Item; type Construction; }
impl<C> Unbounded<C> for ReadyItem<Open, Open> { type Item = ReadyItem<Set, Set>; type Construction = C; }
pub trait AttemptTimeout { type Item; }
impl<W> AttemptTimeout for ReadyItem<W, Open> { type Item = ReadyItem<W, Set>; }

pub struct Handle<T, K, C = Open> { _s: PhantomData<fn() -> (T, K, C)> }
impl<T, K, C> Handle<T, K, C> {
    fn to<K2, C2>(self) -> Handle<T, K2, C2> { Handle { _s: PhantomData } }
    pub fn ready<Args, F: Factory<Args>>(self, _check: F) -> Handle<T, ReadyItem<Open, Open>, C> { self.to() }
}
impl<T, K: Unbounded<C>, C> Handle<T, K, C> {
    pub fn unbounded(self) -> Handle<T, K::Item, K::Construction> { self.to() }
}
impl<T, K: AttemptTimeout, C> Handle<T, K, C> {
    pub fn attempt_timeout(self, _d: Duration) -> Handle<T, K::Item, C> { self.to() }
}

pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool, Construction<Open>> { Handle { _s: PhantomData } }
}
pub struct PgPool;
fn secs(n: u64) -> Duration { Duration::from_secs(n) }

fn main() {
    let mut m = ModuleDef;
    let _after = m.singleton(|| async { PgPool })
        .ready(|| async { Ok::<(), String>(()) })
        .attempt_timeout(secs(2))
        .unbounded();
    let _before = m.singleton(|| async { PgPool })
        .ready(|| async { Ok::<(), String>(()) })
        .unbounded()
        .attempt_timeout(secs(2));
}
