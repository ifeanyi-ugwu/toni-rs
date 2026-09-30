//! What `provide!(K => source)`, `#[inject(K)]` and every injection site expand through.
//!
//! For `provide!`: whether `K` holds the source's type, which binds it as it is, or a trait object,
//! which takes the cast.
//!
//! A macro sees `K` only as a path, and a generic function cannot ask whether `K::Value` is the
//! declared type, so the choice is made by autoref at the expansion site, where both types are
//! concrete: the method on `&Keyed` applies only when `K::Value` is the declared type, and the one
//! on `Keyed` otherwise. A slot holding the declared type then hands it out as the declaration
//! would under its own type, and one holding a trait object hands out `Arc<K::Value>`.

#![doc(hidden)]

use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::di::{Declaration, Key, Under, token_of};
use crate::error::ResolutionError;

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

/// The container key `#[inject(K)]` resolves a field by, checked to hold `V`: the field's type, the
/// type an `Arc<V>` field shares, or the trait object an `Arc<dyn Trait>` or `Vec<Arc<dyn Trait>>`
/// field holds.
pub fn inject_key<K: Key<Value = V>, V: ?Sized>() -> String {
    token_of::<K>()
}

/// What a field or parameter written `Arc<T>` takes from a provider's answer: the shared `Arc<T>`
/// as it is, or a `T` handed out by value, wrapped.
pub fn take_shared<T: 'static>(
    answer: Box<dyn Any + Send>,
    token: &str,
) -> Result<Arc<T>, ResolutionError> {
    match answer.downcast::<Arc<T>>() {
        Ok(shared) => Ok(*shared),
        Err(answer) => answer
            .downcast::<T>()
            .map(|value| Arc::new(*value))
            .map_err(|_| mismatch(token)),
    }
}

/// What a field or parameter written `Arc<dyn Trait>` takes: the `Arc` a slot holding the trait
/// object answers.
pub fn take_object<T: ?Sized + 'static>(
    answer: Box<dyn Any + Send>,
    token: &str,
) -> Result<Arc<T>, ResolutionError> {
    answer
        .downcast::<Arc<T>>()
        .map(|object| *object)
        .map_err(|_| mismatch(token))
}

/// What a field or parameter written as a plain `T` takes: a `T` handed out by value. A shared
/// `Arc<T>` is refused, since the field would hold a copy of it.
pub fn take_value<T: 'static>(
    answer: Box<dyn Any + Send>,
    token: &str,
) -> Result<T, ResolutionError> {
    match answer.downcast::<T>() {
        Ok(value) => Ok(*value),
        Err(answer) if answer.is::<Arc<T>>() => Err(ResolutionError::SharedByValue {
            token: token.to_string(),
            wrote: std::any::type_name::<T>().to_string(),
        }),
        Err(_) => Err(mismatch(token)),
    }
}

/// Whether a type is handed out as one shared instance. `#[injectable]` and `#[websocket_gateway]`
/// shadow this `false` with an inherent const on a singleton or execution-scoped type, read at a
/// plain field or parameter of that type to refuse it where it is written.
pub trait SharedFlag {
    const __ULO_SHARED: bool = false;
}

impl<T: ?Sized> SharedFlag for T {}

fn mismatch(token: &str) -> ResolutionError {
    ResolutionError::TypeMismatch {
        token: token.to_string(),
    }
}
