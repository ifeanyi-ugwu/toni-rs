//! P10b: the design's spelling, `|exec| async move { .. }`, with `exec: &Execution`.
//! Expected: a lifetime error (the returned future borrows the closure's argument).
use std::future::Future;
pub struct Execution { pub name: &'static str }
impl Execution { pub async fn get(&self) -> Result<&'static str, ()> { Ok(self.name) } }
pub async fn execute<F, Fut, R>(f: F) -> R where F: FnOnce(&Execution) -> Fut, Fut: Future<Output = R> {
    let exec = Execution { name: "borrowed" };
    f(&exec).await
}
fn main() { let _ = execute(|exec| async move { exec.get().await }); }
