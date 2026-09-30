//! P08b: whether a `Send + Sync` requirement on `Dep<T>` can carry a `what to write` hint via a
//! blanket-implemented wrapper trait with `on_unimplemented`. Expected: E0277; records whether
//! the wrapper's note is printed or only the auto-trait's own message.
use std::sync::Arc;
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be shared by the container",
    note = "add `Send + Sync` as supertraits of the trait, or spell the site `Dep<dyn Trait + Send + Sync>` on both the binding and the site"
)]
pub trait Shareable {}
impl<T: ?Sized + Send + Sync> Shareable for T {}
pub struct Dep<T: ?Sized + Shareable>(Arc<T>);
pub trait Repo {}
pub struct Svc { repo: Dep<dyn Repo> }
fn main() {}
