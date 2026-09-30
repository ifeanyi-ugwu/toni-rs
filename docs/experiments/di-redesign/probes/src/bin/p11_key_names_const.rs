//! P11: `Key { ty, qualifier, names: &'static KeyNames }` built per `(T, Q)` instantiation.
//! `type_name` is not a `const fn` on stable (first run: "not yet stable as a const fn" at an
//! inline `const` block), and a `static` inside a generic fn is shared across instantiations, so
//! `&'static KeyNames` has no stable spelling. `type_name` returns `&'static str` at runtime, so
//! two `&'static str` fields carry the same information. Expected: compiles, unequal keys print.
use std::any::{type_name, TypeId};
use std::hash::{Hash, Hasher};
#[derive(Clone, Copy)]
pub struct Key { ty: TypeId, qualifier: TypeId, ty_name: &'static str, q_name: &'static str }
impl PartialEq for Key { fn eq(&self, o: &Self) -> bool { self.ty == o.ty && self.qualifier == o.qualifier } }
impl Eq for Key {}
impl Hash for Key { fn hash<H: Hasher>(&self, h: &mut H) { self.ty.hash(h); self.qualifier.hash(h); } }
impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key {
        Key { ty: TypeId::of::<T>(), qualifier: TypeId::of::<Q>(), ty_name: type_name::<T>(), q_name: type_name::<Q>() }
    }
}
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.qualifier == TypeId::of::<()>() { write!(f, "{}", self.ty_name) } else { write!(f, "{} @ {}", self.ty_name, self.q_name) }
    }
}
pub struct PgPool; pub struct Replica; pub trait Plugin {}
fn main() {
    let a = Key::of::<PgPool, ()>(); let b = Key::of::<PgPool, Replica>(); let c = Key::of::<dyn Plugin, ()>();
    println!("{a} | {b} | {c} | a==b {}", a == b);
}
