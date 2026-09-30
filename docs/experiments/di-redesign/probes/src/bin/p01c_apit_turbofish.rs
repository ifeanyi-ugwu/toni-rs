//! P01c: the design writes `also_as::<dyn UserRepo>(|a| a)` with one type argument, so the
//! closure's type must be an argument-position `impl Trait` (explicit generic args beside APIT,
//! stable 1.63). Expected: compiles, and the coercion still happens at the closure's return.
use std::sync::Arc;
pub trait UserRepo: Send + Sync { fn n(&self) -> u8; }
pub struct PgUserRepo; impl UserRepo for PgUserRepo { fn n(&self) -> u8 { 3 } }
pub struct Handle<T>(std::marker::PhantomData<fn() -> T>);
impl<T: Send + Sync + 'static> Handle<T> {
    pub fn also_as<U: ?Sized + Send + Sync + 'static>(self, f: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static) -> Arc<U>
    where T: Default { f(Arc::new(T::default())) }
}
impl Default for PgUserRepo { fn default() -> Self { PgUserRepo } }
fn main() {
    let repo: Arc<dyn UserRepo> = Handle::<PgUserRepo>(Default::default()).also_as::<dyn UserRepo>(|a| a);
    println!("apit turbofish: {}", repo.n());
}
