//! P08: `dyn Repo` and `dyn Repo + Send + Sync` are distinct `TypeId`s even when `Repo` has
//! the auto traits as supertraits, so they are two keys. Also: the `type_name` strings the
//! wiring error would print for each. Expected: two ids, two names.
use std::any::{type_name, TypeId};
pub trait Repo: Send + Sync {}
fn main() {
    let a = TypeId::of::<dyn Repo>();
    let b = TypeId::of::<dyn Repo + Send + Sync>();
    println!("same TypeId: {}", a == b);
    println!("{} | {}", type_name::<dyn Repo>(), type_name::<dyn Repo + Send + Sync>());
}
