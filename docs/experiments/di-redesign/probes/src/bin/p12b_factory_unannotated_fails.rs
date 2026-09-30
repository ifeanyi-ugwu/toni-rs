//! P12b: the same `singleton(f)` with an unannotated closure parameter. Expected: the site type
//! cannot be inferred (E0282/E0283), so every factory parameter must be written with its type.
use std::future::Future;
use std::sync::Arc;
pub trait Site: Sized + Send + 'static {}
pub struct Dep<T: ?Sized>(pub Arc<T>);
impl<T: ?Sized + Send + Sync + 'static> Site for Dep<T> {}
pub trait Factory<Args> { type Output; }
impl<F, Fut, A> Factory<(A,)> for F where A: Site, F: Fn(A) -> Fut, Fut: Future { type Output = Fut::Output; }
pub fn singleton<Args, F: Factory<Args>>(_f: F) {}
pub struct PgPool;
fn main() { singleton(|cfg| async move { let _ = cfg; PgPool }); }
