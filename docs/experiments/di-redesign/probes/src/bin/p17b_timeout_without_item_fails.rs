//! P17b: `.timeout(..)` on a handle whose last call wrote no timed item (`NoItem`). Expected:
//! fails, E0599 — the method exists, but `NoItem: HasTimeout` is unsatisfied. Records whether
//! the `on_unimplemented` text reaches an E0599 note.
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

pub trait Factory<Args>: Send + Sync + 'static {}
impl<F, Fut> Factory<()> for F where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future + Send {}

pub struct NoItem;
pub struct HookItem;
#[diagnostic::on_unimplemented(
    message = "`.timeout(..)` needs an item to apply to",
    note = "write it after `.ready(..)` or after an `on_*` hook"
)]
pub trait HasTimeout {}
impl HasTimeout for HookItem {}

pub struct Handle<T, K = NoItem> { _s: PhantomData<fn() -> (T, K)> }
impl<T, K: HasTimeout> Handle<T, K> {
    pub fn timeout(self, _d: Duration) -> Self { self }
}
pub struct ModuleDef;
impl ModuleDef {
    pub fn singleton<Args, F: Factory<Args>>(&mut self, _f: F) -> Handle<PgPool> { Handle { _s: PhantomData } }
}
pub struct PgPool;

fn main() {
    let mut m = ModuleDef;
    let _h = m.singleton(|| async { PgPool }).timeout(Duration::from_secs(1));
}
