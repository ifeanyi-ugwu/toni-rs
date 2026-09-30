//! P05b: a site's `read` holds `&Resolver<'_>` (and through it `&Execution`) across an await in
//! a `Send` future. A `Cell` inside the execution, standing for any `!Sync` seeded input,
//! makes the constructor future `!Send`. Expected: E0277 naming the `Cell`.
use std::cell::Cell;
use std::future::Future;
pub struct Execution { pub counter: Cell<u32> }
pub struct Resolver<'e> { pub exec: &'e Execution }
pub trait Site: Sized + Send + 'static {
    fn read(r: &Resolver<'_>) -> impl Future<Output = Self> + Send;
}
pub struct Marker;
impl Site for Marker {
    async fn read(r: &Resolver<'_>) -> Self { std::future::ready(()).await; r.exec.counter.set(1); Marker }
}
fn main() {}
