//! P19: a handle item's bound is written once. The state parameter names the item written last
//! and whether its bound is written: `.timeout(..)` and `.unbounded()` exist on an item in `Open`
//! and return it in `Set`, which has neither. A readiness item has two slots, the whole check and
//! one attempt, each written once in either order; `.unbounded()` takes both and exists only while
//! both are open. The handle keeps the construction's state as a third parameter, so `also_as` and
//! `qualified` return to the binding as it was left and its bound is written once wherever the
//! handle is on it. Expected: compiles; prints each item's bounds.
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

pub struct Dep<T: ?Sized>(pub Arc<T>);
pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}
impl<F, Fut, A> Factory<(A,)> for F where F: Fn(A) -> Fut + Send + Sync + 'static, Fut: Future + Send {}

#[derive(Debug, Clone, Copy, Default)]
pub enum Bound { #[default] Default, After(Duration), Unbounded }
#[derive(Debug, Default)]
pub struct Item { pub name: &'static str, pub bound: Bound, pub attempt: Bound, pub retries: u32 }

pub struct Open;
pub struct Set;
pub struct Construction<B>(PhantomData<B>);
pub struct HookItem<B>(PhantomData<B>);
pub struct ReadyItem<W, A>(PhantomData<(W, A)>);

/// Which `Item` the state's bound methods write: the construction is the first item, every
/// other state the last.
pub trait Slot { fn slot(items: &mut [Item]) -> &mut Item; }
impl<B> Slot for Construction<B> { fn slot(items: &mut [Item]) -> &mut Item { items.first_mut().expect("the binding") } }
impl<B> Slot for HookItem<B> { fn slot(items: &mut [Item]) -> &mut Item { items.last_mut().expect("an item") } }
impl<W, A> Slot for ReadyItem<W, A> { fn slot(items: &mut [Item]) -> &mut Item { items.last_mut().expect("an item") } }

/// `.timeout(..)` on an item in `Open`: the item's next state, and the construction's.
pub trait Timeout<C>: Slot { type Item; type Construction; }
impl Timeout<Open> for Construction<Open> { type Item = Construction<Set>; type Construction = Set; }
impl<C> Timeout<C> for HookItem<Open> { type Item = HookItem<Set>; type Construction = C; }
impl<A, C> Timeout<C> for ReadyItem<Open, A> { type Item = ReadyItem<Set, A>; type Construction = C; }

/// `.unbounded()`: on readiness only while both slots are open, and it closes both.
pub trait Unbounded<C>: Slot { type Item; type Construction; }
impl Unbounded<Open> for Construction<Open> { type Item = Construction<Set>; type Construction = Set; }
impl<C> Unbounded<C> for HookItem<Open> { type Item = HookItem<Set>; type Construction = C; }
impl<C> Unbounded<C> for ReadyItem<Open, Open> { type Item = ReadyItem<Set, Set>; type Construction = C; }

/// `.attempt_timeout(..)`: the readiness item's second slot, written once.
pub trait AttemptTimeout: Slot { type Item; }
impl<W> AttemptTimeout for ReadyItem<W, Open> { type Item = ReadyItem<W, Set>; }

pub struct Handle<T, K, C = Open> { pub items: Vec<Item>, _s: PhantomData<fn() -> (T, K, C)> }
impl<T, K, C> Handle<T, K, C> {
    fn to<K2, C2>(self) -> Handle<T, K2, C2> { Handle { items: self.items, _s: PhantomData } }
    fn with<K2>(mut self, name: &'static str) -> Handle<T, K2, C> {
        self.items.push(Item { name, ..Default::default() });
        self.to()
    }
    pub fn ready<Args, F: Factory<Args>>(self, _check: F) -> Handle<T, ReadyItem<Open, Open>, C> { self.with("ready") }
    pub fn on_init<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem<Open>, C> { self.with("on_init") }
    pub fn on_destroy<Args, F: Factory<Args>>(self, _f: F) -> Handle<T, HookItem<Open>, C> { self.with("on_destroy") }
    /// Back to the binding, in the construction state the handle remembers.
    pub fn also_as<U: ?Sized + 'static>(self) -> Handle<T, Construction<C>, C> { self.to() }
    pub fn qualified<Q: 'static>(self) -> Handle<T, Construction<C>, C> { self.to() }
}
impl<T, K: Timeout<C>, C> Handle<T, K, C> {
    pub fn timeout(mut self, d: Duration) -> Handle<T, K::Item, K::Construction> {
        K::slot(&mut self.items).bound = Bound::After(d);
        self.to()
    }
}
impl<T, K: Unbounded<C>, C> Handle<T, K, C> {
    pub fn unbounded(mut self) -> Handle<T, K::Item, K::Construction> {
        let item = K::slot(&mut self.items);
        item.bound = Bound::Unbounded;
        item.attempt = Bound::Unbounded;
        self.to()
    }
}
impl<T, K: AttemptTimeout, C> Handle<T, K, C> {
    pub fn attempt_timeout(mut self, d: Duration) -> Handle<T, K::Item, C> {
        K::slot(&mut self.items).attempt = Bound::After(d);
        self.to()
    }
}
/// Not bounds: available in every readiness state, `Set` and `Unbounded` included.
impl<T, W, A, C> Handle<T, ReadyItem<W, A>, C> {
    pub fn retries(mut self, n: u32) -> Self { self.items.last_mut().expect("the check").retries = n; self }
    pub fn backoff(self, _d: Duration) -> Self { self }
}

pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool, Construction<Open>> {
        Handle { items: vec![Item { name: "construction", ..Default::default() }], _s: PhantomData }
    }
}

pub struct PgPool;
pub struct Replica;
fn secs(n: u64) -> Duration { Duration::from_secs(n) }

fn main() {
    let mut m = ModuleDef;
    let h1 = m.singleton(|| async { PgPool })
        .also_as::<dyn Send>()
        .timeout(secs(20))                                                      // the construction, after a return to the binding
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) })
        .retries(5).attempt_timeout(secs(2)).timeout(secs(10)).backoff(secs(1)) // both slots; retries/backoff around them
        .on_destroy(|_pool: Dep<PgPool>| async move {}).unbounded()
        .qualified::<Replica>();                                                // back to the binding, construction already `Set`
    let h2 = m.singleton(|| async { PgPool })
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) })
        .timeout(secs(10)).attempt_timeout(secs(2))                             // the other order
        .on_init(|_pool: Dep<PgPool>| async move {}).timeout(secs(1));
    let h3 = m.singleton(|| async { PgPool })
        .ready(|_pool: Dep<PgPool>| async move { Ok::<(), String>(()) })
        .unbounded().retries(3)                                                 // retries keeps its meaning under `.unbounded()`
        .on_destroy(|_pool: Dep<PgPool>| async move {});                        // left at `Default`
    println!("h1: {:?}", h1.items);
    println!("h2: {:?}", h2.items);
    println!("h3: {:?}", h3.items);
}
