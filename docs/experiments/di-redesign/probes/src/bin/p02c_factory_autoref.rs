//! P02c: autoref rank over the closure's concrete type, the mechanism a macro could use when
//! it sees the call site. `(&Probe(f)).kind()` reaches the fallible arm for a `Result` future
//! and falls back to the plain arm otherwise. Expected: compiles, prints "fallible" then "plain".
use std::future::Future;
pub struct Probe<F>(pub F);
pub trait FallibleKind { fn kind(&self) -> &'static str { "fallible" } }
pub trait PlainKind { fn kind(&self) -> &'static str { "plain" } }
impl<F, Fut, T, E> FallibleKind for Probe<F> where F: Fn() -> Fut, Fut: Future<Output = Result<T, E>> {}
impl<F> PlainKind for &Probe<F> {}
fn main() {
    let fallible = || async { Ok::<u32, String>(1) };
    let plain = || async { 1u32 };
    println!("{}", (&Probe(fallible)).kind());
    println!("{}", (&Probe(plain)).kind());
}
