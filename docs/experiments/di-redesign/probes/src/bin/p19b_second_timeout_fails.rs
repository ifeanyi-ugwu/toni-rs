//! P19b: a second bound on one item. Two sites: `.timeout` twice on the readiness check, and the
//! construction's `.timeout` after `also_as` returned to a binding whose bound is already written.
//! Expected: fails, two E0599, each on the second `timeout`, naming the handle type the method
//! exists on.
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}

pub struct Open;
pub struct Set;
pub struct Construction<B>(PhantomData<B>);
pub struct ReadyItem<W, A>(PhantomData<(W, A)>);

pub trait Timeout<C> { type Item; type Construction; }
impl Timeout<Open> for Construction<Open> { type Item = Construction<Set>; type Construction = Set; }
impl<A, C> Timeout<C> for ReadyItem<Open, A> { type Item = ReadyItem<Set, A>; type Construction = C; }

pub struct Handle<T, K, C = Open> { _s: PhantomData<fn() -> (T, K, C)> }
impl<T, K, C> Handle<T, K, C> {
    fn to<K2, C2>(self) -> Handle<T, K2, C2> { Handle { _s: PhantomData } }
    pub fn ready<Args, F: Factory<Args>>(self, _check: F) -> Handle<T, ReadyItem<Open, Open>, C> { self.to() }
    pub fn also_as<U: ?Sized + 'static>(self) -> Handle<T, Construction<C>, C> { self.to() }
}
impl<T, K: Timeout<C>, C> Handle<T, K, C> {
    pub fn timeout(self, _d: Duration) -> Handle<T, K::Item, K::Construction> { self.to() }
}

pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool, Construction<Open>> { Handle { _s: PhantomData } }
}
pub struct PgPool;
fn secs(n: u64) -> Duration { Duration::from_secs(n) }

fn main() {
    let mut m = ModuleDef;
    let _twice = m.singleton(|| async { PgPool })
        .ready(|| async { Ok::<(), String>(()) })
        .timeout(secs(10))
        .timeout(secs(5));
    let _again = m.singleton(|| async { PgPool })
        .timeout(secs(20))
        .ready(|| async { Ok::<(), String>(()) })
        .also_as::<dyn Send>()
        .timeout(secs(30));
}
