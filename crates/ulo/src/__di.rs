//! What `provide!(K => source)` expands through: whether `K` holds the source's type, which binds
//! it as it is, or a trait object, which takes the cast.
//!
//! A macro sees `K` only as a path, and a generic function cannot ask whether `K::Value` is the
//! declared type, so the choice is made by autoref at the expansion site, where both types are
//! concrete: the method on `&Keyed` applies only when `K::Value` is the declared type, and the one
//! on `Keyed` otherwise. A slot holding the declared type then hands it out as the declaration
//! would under its own type, and one holding a trait object hands out `Arc<K::Value>`.

#![doc(hidden)]

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::di::{Declaration, Key, Under, token_of};

pub struct Keyed<K, D> {
    declaration: Cell<Option<D>>,
    _key: PhantomData<fn() -> K>,
}

impl<K, D> Keyed<K, D> {
    fn take(&self) -> D {
        self.declaration
            .take()
            .unwrap_or_else(|| panic!("a keyed declaration is bound once"))
    }
}

pub fn keyed<K: Key, D: Declaration>(declaration: D) -> Keyed<K, D> {
    Keyed {
        declaration: Cell::new(Some(declaration)),
        _key: PhantomData,
    }
}

pub trait BindAsIs {
    type Declared;
    type Held: ?Sized;
    type Output;
    fn bind(&self, cast: fn(Arc<Self::Declared>) -> Arc<Self::Held>) -> Self::Output;
}

impl<K: Key<Value = D::Output>, D: Declaration> BindAsIs for &Keyed<K, D> {
    type Declared = D::Output;
    type Held = K::Value;
    type Output = Under<D>;

    fn bind(&self, _cast: fn(Arc<D::Output>) -> Arc<K::Value>) -> Under<D> {
        Under::new(self.take(), token_of::<K>())
    }
}

pub trait BindCast {
    type Declared;
    type Held: ?Sized;
    type Output;
    fn bind(&self, cast: fn(Arc<Self::Declared>) -> Arc<Self::Held>) -> Self::Output;
}

impl<K, D> BindCast for Keyed<K, D>
where
    K: Key,
    K::Value: Send + Sync,
    D: Declaration,
{
    type Declared = D::Output;
    type Held = K::Value;
    type Output = D::Held<K::Value>;

    fn bind(&self, cast: fn(Arc<D::Output>) -> Arc<K::Value>) -> D::Held<K::Value> {
        self.take().under_key_with::<K>(cast)
    }
}
