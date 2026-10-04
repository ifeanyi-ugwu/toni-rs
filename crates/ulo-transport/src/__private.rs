//! Support for the code a transport's handler attribute generates through `ulo-handler-codegen`.
//! Not part of the public API: names and shapes here change with the codegen.

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;

use ulo::__private::{HandlerParam, handler_param};
use ulo::{BoxError, Dep, Dependencies, ExecutionRef, FromContainer, Transport};

use crate::error::{CallError, ErrorKind};
use crate::extract::{ExtractError, FromCall, Injected};
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

    /// What the parameter reads from the container, for the wiring pass, every read labelled
    /// `param`: a wiring report prints ``param `name` `` for a parameter the signature names and
    /// `param #n` for a destructuring pattern, `n` its position in the signature.
    fn dependencies(d: &mut Dependencies, param: HandlerParam);

    /// The parameter, or the failure for the error handlers: a `CallError` of the
    /// `ExtractError`'s kind, holding it as its source (transports DESIGN §2.2).
    fn extract(cx: &T::Cx) -> impl Future<Output = Result<Self, BoxError>> + Send;
}

impl<T: Transport, P: FromCall<T>> Param<T, ViaCall> for P {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;

    fn dependencies(d: &mut Dependencies, param: HandlerParam) {
        handler_param(d, param, P::dependencies);
    }

    async fn extract(cx: &T::Cx) -> Result<Self, BoxError> {
        P::from_call(cx).await.map_err(boxed)
    }
}

impl<T, S> Param<T, ViaContainer> for S
where
    T: Transport,
    T::Cx: AsRef<ExecutionRef>,
    S: FromContainer,
{
    const CONSUMES_BODY: bool = false;

    fn dependencies(d: &mut Dependencies, param: HandlerParam) {
        handler_param(d, param, |d| {
            d.add::<S>();
        });
    }

    async fn extract(cx: &T::Cx) -> Result<Self, BoxError> {
        <Injected<S> as FromCall<T>>::from_call(cx).await.map(|injected| injected.0).map_err(boxed)
    }
}

fn boxed(err: ExtractError) -> BoxError {
    BoxError::from(CallError::from_extract(err))
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
        match self.take()? {
            Ok(value) => reply::<T, V>(value, cx),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        }
    }
}

impl<T: Transport, V: IntoReply<T>, E: Into<BoxError>> ViaBoxError<T> for &IntoReplyProbe<T, Result<V, E>> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError> {
        match self.take()? {
            Ok(value) => reply::<T, V>(value, cx),
            Err(err) => Err(err.into()),
        }
    }
}

impl<T: Transport, V: IntoReply<T>> ViaValue<T> for IntoReplyProbe<T, V> {
    fn into_reply(&self, cx: &T::Cx) -> Result<T::Reply, BoxError> {
        reply::<T, V>(self.take()?, cx)
    }
}

impl<T, V> IntoReplyProbe<T, V> {
    /// The value, once. The generated code converts each probe once; a second call is answered
    /// as an internal error rather than a panic.
    fn take(&self) -> Result<V, BoxError> {
        self.value.take().ok_or_else(|| BoxError::from(CallError::new(ErrorKind::Internal, "internal error")))
    }
}

/// The value side of every arm: a conversion failure reaches the error handlers as a `CallError`
/// of kind `Internal` holding the `IntoReplyError`, as an extraction failure does.
fn reply<T: Transport, V: IntoReply<T>>(value: V, cx: &T::Cx) -> Result<T::Reply, BoxError> {
    <V as IntoReply<T>>::into_reply(value, cx).map_err(|err| BoxError::from(CallError::from(err)))
}

/// What `#[derive(Validate)]` generates calls.
pub mod validate {
    /// The length a `length(..)` rule checks, chosen by autoref:
    /// `(&&LengthProbe(&self.field)).length()` with both traits imported. Text, anything
    /// `AsRef<str>`, is counted in characters, so a `max` bounds what a person typed rather than its
    /// UTF-8 encoding; a collection whose borrowing iterator knows its length is counted in items.
    pub struct LengthProbe<'a, T: ?Sized>(pub &'a T);

    pub trait ViaChars {
        fn length(&self) -> usize;
    }

    impl<T: ?Sized + AsRef<str>> ViaChars for &LengthProbe<'_, T> {
        fn length(&self) -> usize {
            <T as AsRef<str>>::as_ref(self.0).chars().count()
        }
    }

    pub trait ViaItems {
        fn length(&self) -> usize;
    }

    impl<'a, T: ?Sized> ViaItems for LengthProbe<'a, T>
    where
        &'a T: IntoIterator,
        <&'a T as IntoIterator>::IntoIter: ExactSizeIterator,
    {
        fn length(&self) -> usize {
            <&'a T as IntoIterator>::into_iter(self.0).len()
        }
    }

    /// The `email` rule, a shape check rather than RFC 5322: one `@`; a local part of 1 to 64
    /// bytes with no whitespace or control character; a domain of two or more dot-separated
    /// labels, each non-empty, of letters, digits and `-`, neither starting nor ending with `-`;
    /// 254 bytes in all, the longest address SMTP carries (RFC 5321).
    pub fn is_email(text: &str) -> bool {
        let Some((local, domain)) = text.split_once('@') else {
            return false;
        };
        if domain.contains('@') || local.is_empty() || local.len() > 64 || text.len() > 254 {
            return false;
        }
        if local.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        let mut labels = 0;
        for label in domain.split('.') {
            let shaped = !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_alphanumeric() || c == '-');
            if !shaped {
                return false;
            }
            labels += 1;
        }
        labels >= 2
    }
}
