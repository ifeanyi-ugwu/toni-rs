//! P01b: the same `|a| a` when the type does not implement the trait. Expected: E0277 at the
//! closure, naming the missing impl. Records what the user reads.
use std::sync::Arc;
pub trait Cache: Send + Sync {}
pub struct NotCache;
pub fn also_as<T, U, F>(_f: F) where U: ?Sized, F: FnOnce(Arc<T>) -> Arc<U> {}
fn main() {
    also_as::<NotCache, dyn Cache, _>(|a| a);
}
