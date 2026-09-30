//! P02b: the marker-parameter escape from E0119. The impls no longer overlap, but a
//! `Result<T, E>` output satisfies both, so `M` is ambiguous at the call. Expected: E0283/E0282.
use std::future::Future;
pub struct Plain; pub struct Fallible;
pub trait FactoryOutput<M> { type Value; }
impl<T> FactoryOutput<Plain> for T { type Value = T; }
impl<T, E> FactoryOutput<Fallible> for Result<T, E> { type Value = T; }
pub fn singleton<F, Fut, O, M>(_f: F) where F: Fn() -> Fut, Fut: Future<Output = O>, O: FactoryOutput<M> {}
fn main() {
    singleton(|| async { 1u32 });
    singleton(|| async { Ok::<u32, String>(1) });
}
