//! P23b (§2.2): `Param<T>` for `Option<P: Param<T>>` beside `Param<T>` for `Option<S:
//! FromContainer>`, the reading of "one concrete impl each" that puts `Option` on the
//! `FromContainer` side while `Option<Json<T>>` still needs the `Param` side. Expected: E0119,
//! conflicting implementations for `Option<_>`.
use std::marker::PhantomData;

pub trait Transport: 'static {}
pub struct Http;
impl Transport for Http {}

pub trait FromContainer: Sized + Send + 'static {}
pub trait Param<T: Transport>: Sized + Send + 'static {}

pub struct Dep<U>(PhantomData<fn() -> U>);
impl<U: 'static> FromContainer for Dep<U> {}
impl<T: Transport, U: 'static> Param<T> for Dep<U> {}

impl<T: Transport, P: Param<T>> Param<T> for Option<P> {}
impl<T: Transport, S: FromContainer> Param<T> for Option<S> {}

fn main() {}
