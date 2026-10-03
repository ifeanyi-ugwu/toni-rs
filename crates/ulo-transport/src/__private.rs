//! Support for the code a transport's handler attribute generates through `ulo-handler-codegen`.
//! Not part of the public API: names and shapes here change with the codegen.

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;

use ulo::{BoxError, Dep, Dependencies, ExecutionRef, FromContainer, Transport};

use crate::error::CallError;
use crate::extract::{FromCall, Injected};
use crate::reply::IntoReply;

/// The marker of a parameter read from the call.
pub enum ViaCall {}

/// The marker of a parameter read from the container.
pub enum ViaContainer {}

/// Every handler parameter, read by type: once for every `FromCall<T>` type and once for every
/// `FromContainer` type. The generated code names it `<P as Param<T, _>>` and the marker is
/// inferred from the one impl that applies, which works in a constant's initializer, where the
/// pairwise `CONSUMES_BODY` assertions sit, as well as in the call.
///
/// No type is both `FromCall<T>` and `FromContainer`, so the marker is never ambiguous: the core's
/// injection points are `FromContainer` alone and reach a handler through the second impl.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be built from a `{T}` call",
    label = "a handler parameter is read from the call or from the container",
    note = "a transport implements `FromCall` for its own extractors; `FromContainer` types (`Dep`, `Ext`, ...) are accepted on every transport"
)]
pub trait Param<T: Transport, M>: Sized + Send + 'static {
    const CONSUMES_BODY: bool;

    fn dependencies(d: &mut Dependencies);

    /// The parameter, or the failure boxed as an `ExtractError` for the error handlers.
    fn extract(cx: &T::Cx) -> impl Future<Output = Result<Self, BoxError>> + Send;
}

impl<T: Transport, P: FromCall<T>> Param<T, ViaCall> for P {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;

    fn dependencies(d: &mut Dependencies) {
        P::dependencies(d);
    }

    async fn extract(cx: &T::Cx) -> Result<Self, BoxError> {
        P::from_call(cx).await.map_err(BoxError::from)
    }
}

impl<T, S> Param<T, ViaContainer> for S
where
    T: Transport,
    T::Cx: AsRef<ExecutionRef>,
    S: FromContainer,
{
    const CONSUMES_BODY: bool = false;

    fn dependencies(d: &mut Dependencies) {
        <Injected<S> as FromCall<T>>::dependencies(d);
    }

    async fn extract(cx: &T::Cx) -> Result<Self, BoxError> {
        <Injected<S> as FromCall<T>>::from_call(cx).await.map(|injected| injected.0).map_err(BoxError::from)
    }
}

/// The controller `C`, from the call's execution routed to its module: the stored singleton, the
/// execution's cached instance, or a fresh transient.
pub async fn controller<C, T>(cx: &T::Cx) -> Result<Dep<C>, BoxError>
where
    C: Send + Sync + 'static,
    T: Transport,
    T::Cx: AsRef<ExecutionRef>,
{
    let exec: &ExecutionRef = cx.as_ref();
    exec.get::<C>().await.map_err(BoxError::from)
}

/// The reply probe (transports DESIGN §2.3):
/// `(&&&IntoReplyProbe::<T, _>::new(out)).into_reply(&cx)`.
///
/// Three arms, each one reference deeper than the priority reads, since method lookup tries the
/// impl on `&&IntoReplyProbe` first, then `&IntoReplyProbe`, then the bare type, and the first
/// whose impl where-clauses hold wins. The arms are not disjoint, a `CallError` being an `Error`;
/// the order decides. The transport is a parameter of the probe type rather than of the method,
/// since lookup checks an impl's where-clauses and never a method's, and `V: IntoReply<T>` is one
/// of them. The methods take `&self`, which the ranking needs, so the value sits in a `Cell`.
pub struct IntoReplyProbe<T, V> {
    value: Cell<Option<V>>,
    _t: PhantomData<fn() -> T>,
}

impl<T, V> IntoReplyProbe<T, V> {
    pub fn new(value: V) -> Self {
        IntoReplyProbe { value: Cell::new(Some(value)), _t: PhantomData }
    }
}

/// `Result<V, E>` with `E: Into<CallError>`: every `Classify` error, a `CallError` itself, and a
/// user type with its own `From<MyErr> for CallError`. `Err` becomes
/// `BoxError::from(CallError::from(e))`.
pub trait ViaCallError<T: Transport> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError>;
}

/// `Result<V, E>` with `E: Into<BoxError>`: `Err` boxed unchanged.
pub trait ViaBoxError<T: Transport> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError>;
}

/// Any `V: IntoReply<T>`: answered as the value.
pub trait ViaValue<T: Transport> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError>;
}

impl<T: Transport, V: IntoReply<T>, E: Into<CallError>> ViaCallError<T> for &&IntoReplyProbe<T, Result<V, E>> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError> {
        let _ = cx;
        todo!("take the value; `Ok(v)` → `v.into_reply(cx)`, boxed on failure; `Err(e)` → `BoxError::from(CallError::from(e))`")
    }
}

impl<T: Transport, V: IntoReply<T>, E: Into<BoxError>> ViaBoxError<T> for &IntoReplyProbe<T, Result<V, E>> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError> {
        let _ = cx;
        todo!("take the value; `Ok(v)` → `v.into_reply(cx)`, boxed on failure; `Err(e)` → `e.into()`")
    }
}

impl<T: Transport, V: IntoReply<T>> ViaValue<T> for IntoReplyProbe<T, V> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError> {
        let _ = cx;
        todo!("take the value; `v.into_reply(cx)`, boxed on failure")
    }
}
