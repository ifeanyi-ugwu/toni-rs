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

/// What a site of type `Arc<T>` takes from a provider's answer: the shared `Arc<T>` as it is, or a
/// `T` handed out by value, wrapped.
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

/// What a site of type `Arc<dyn Trait>` takes: the `Arc` a slot holding the trait object answers.
pub fn take_object<T: ?Sized + 'static>(
    answer: Box<dyn Any + Send>,
    token: &str,
) -> Result<Arc<T>, ResolutionError> {
    answer
        .downcast::<Arc<T>>()
        .map(|object| *object)
        .map_err(|_| mismatch(token))
}

/// What a site of a plain type `T` takes: a `T` handed out by value. A shared `Arc<T>` is refused,
/// since the site would hold a copy of it.
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

/// An injection site of type `F`, whose key, answer, value read and `#[inject(K)]` check follow
/// `F`'s type rather than its spelling, so an alias or a renamed `Arc` reads as the `Arc` it
/// names. The macros call each method as `(&&&Site::<F>::new()).m()`, where autoref ranks the
/// rungs: an `Arc` of a sized type, an `Arc` of anything, a collection of an unsized `Send + Sync`
/// element, a trait object in practice, then any other type, which is read by value. A `Vec<Arc<T>>` whose `T` is sized, `str` or a slice is not
/// a collection: it takes the first rung's rank to be read by value.
pub struct Site<F: ?Sized>(PhantomData<fn() -> Box<F>>);

impl<F: ?Sized> Site<F> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Site(PhantomData)
    }
}

/// The key a trait-object key names, checked against what the site holds.
pub trait SameAs<T: ?Sized> {}

impl<T: ?Sized> SameAs<T> for T {}

/// The first rung: `Arc<T>` of a sized `T`, keyed by `T`, taking a shared answer or wrapping a
/// value.
pub trait SiteShared {
    type Held;
    type Taken;
    fn key(&self) -> String;
    fn take(
        &self,
        answer: Box<dyn Any + Send>,
        token: &str,
    ) -> Result<Self::Taken, ResolutionError>;
    fn value_read(&self, token: String) -> Option<(String, &'static str)>;
    fn inject_key<K: Key<Value = Self::Held>>(&self) -> String {
        token_of::<K>()
    }
    fn inject_dyn<D: ?Sized + SameAs<Self::Held> + 'static>(&self) -> String {
        token_of::<D>()
    }
}

impl<T: 'static> SiteShared for &&Site<Arc<T>> {
    type Held = T;
    type Taken = Arc<T>;
    fn key(&self) -> String {
        token_of::<T>()
    }
    fn take(&self, answer: Box<dyn Any + Send>, token: &str) -> Result<Arc<T>, ResolutionError> {
        take_shared::<T>(answer, token)
    }
    fn value_read(&self, _token: String) -> Option<(String, &'static str)> {
        None
    }
}

/// Beside the first rung: `Vec<Arc<T>>` whose `T` is sized, `str` or a slice, read as any other
/// type is rather than as a collection.
pub trait SiteValueVec {
    type Held;
    type Taken;
    fn key(&self) -> String;
    fn take(
        &self,
        answer: Box<dyn Any + Send>,
        token: &str,
    ) -> Result<Self::Taken, ResolutionError>;
    fn value_read(&self, token: String) -> Option<(String, &'static str)>;
    fn inject_key<K: Key<Value = Self::Held>>(&self) -> String {
        token_of::<K>()
    }
    fn inject_dyn<D: ?Sized + SameAs<Self::Held> + 'static>(&self) -> String {
        token_of::<D>()
    }
}

macro_rules! value_vec {
    ($([$($generic:tt)*] $element:ty;)*) => {$(
        impl<$($generic)*> SiteValueVec for &&Site<Vec<Arc<$element>>> {
            type Held = Vec<Arc<$element>>;
            type Taken = Vec<Arc<$element>>;
            fn key(&self) -> String {
                token_of::<Vec<Arc<$element>>>()
            }
            fn take(
                &self,
                answer: Box<dyn Any + Send>,
                token: &str,
            ) -> Result<Vec<Arc<$element>>, ResolutionError> {
                take_value::<Vec<Arc<$element>>>(answer, token)
            }
            fn value_read(&self, token: String) -> Option<(String, &'static str)> {
                Some((token, std::any::type_name::<Vec<Arc<$element>>>()))
            }
        }
    )*};
}

value_vec! {
    [T: 'static] T;
    [] str;
    [U: 'static] [U];
}

/// The second rung: `Arc<T>` of any `T`, a trait object included, taking the `Arc` a slot holding
/// it answers.
pub trait SiteObject {
    type Held: ?Sized;
    type Taken;
    fn key(&self) -> String;
    fn take(
        &self,
        answer: Box<dyn Any + Send>,
        token: &str,
    ) -> Result<Self::Taken, ResolutionError>;
    fn value_read(&self, token: String) -> Option<(String, &'static str)>;
    fn inject_key<K: Key<Value = Self::Held>>(&self) -> String {
        token_of::<K>()
    }
    fn inject_dyn<D: ?Sized + SameAs<Self::Held> + 'static>(&self) -> String {
        token_of::<D>()
    }
}

impl<T: ?Sized + 'static> SiteObject for &&&Site<Arc<T>> {
    type Held = T;
    type Taken = Arc<T>;
    fn key(&self) -> String {
        token_of::<T>()
    }
    fn take(&self, answer: Box<dyn Any + Send>, token: &str) -> Result<Arc<T>, ResolutionError> {
        take_object::<T>(answer, token)
    }
    fn value_read(&self, _token: String) -> Option<(String, &'static str)> {
        None
    }
}

/// The third rung: the collection `Vec<Arc<T>>` of an unsized `Send + Sync` element, a trait object
/// in practice, keyed by its element type.
pub trait SiteCollection {
    type Held: ?Sized;
    type Taken;
    fn key(&self) -> String;
    fn take(
        &self,
        answer: Box<dyn Any + Send>,
        token: &str,
    ) -> Result<Self::Taken, ResolutionError>;
    fn value_read(&self, token: String) -> Option<(String, &'static str)>;
    fn inject_key<K: Key<Value = Self::Held>>(&self) -> String {
        token_of::<K>()
    }
    fn inject_dyn<D: ?Sized + SameAs<Self::Held> + 'static>(&self) -> String {
        token_of::<D>()
    }
}

impl<T: ?Sized + Send + Sync + 'static> SiteCollection for &Site<Vec<Arc<T>>> {
    type Held = T;
    type Taken = Vec<Arc<T>>;
    fn key(&self) -> String {
        token_of::<T>()
    }
    fn take(
        &self,
        answer: Box<dyn Any + Send>,
        token: &str,
    ) -> Result<Vec<Arc<T>>, ResolutionError> {
        take_collection::<T>(answer, token)
    }
    fn value_read(&self, _token: String) -> Option<(String, &'static str)> {
        None
    }
}

/// The last rung: any other type, keyed by itself and read by value.
impl<F: 'static> Site<F> {
    pub fn key(&self) -> String {
        token_of::<F>()
    }
    pub fn take(&self, answer: Box<dyn Any + Send>, token: &str) -> Result<F, ResolutionError> {
        take_value::<F>(answer, token)
    }
    pub fn value_read(&self, token: String) -> Option<(String, &'static str)> {
        Some((token, std::any::type_name::<F>()))
    }
    pub fn inject_key<K: Key<Value = F>>(&self) -> String {
        token_of::<K>()
    }
    pub fn inject_dyn<D: ?Sized + SameAs<F> + 'static>(&self) -> String {
        token_of::<D>()
    }
}

/// The tokens a factory's plain fields and parameters read, each with the type it is written as.
pub fn value_reads<const N: usize>(
    reads: [Option<(String, &'static str)>; N],
) -> Vec<(String, &'static str)> {
    reads.into_iter().flatten().collect()
}

/// What a collection site takes: the items a collection provider answers erased, each read back as
/// the element type `provide!` stored it as.
pub fn take_collection<T: ?Sized + Send + Sync + 'static>(
    answer: Box<dyn Any + Send>,
    token: &str,
) -> Result<Vec<Arc<T>>, ResolutionError> {
    let items = answer
        .downcast::<Vec<Arc<dyn Any + Send + Sync>>>()
        .map_err(|_| mismatch(token))?;
    items
        .into_iter()
        .map(|item| {
            Arc::downcast::<Arc<T>>(item)
                .map(|wrapped| (*wrapped).clone())
                .map_err(|_| mismatch(token))
        })
        .collect()
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
