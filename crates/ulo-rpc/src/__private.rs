//! Support for the code `ulo-rpc-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.
//!
//! The attributes build their `ulo_handler_codegen::Paths` with `transport: ::ulo_rpc`, so the
//! generated call names `::ulo_rpc::__private::{Param, controller}`, re-exported below from
//! `ulo-transport`, and this module's own reply probe in place of `ulo-transport`'s. A handler
//! returning a bare `Invoice` or an `impl Stream` cannot go through `ulo-transport`'s probe: its
//! value arm asks `V: IntoReply<Rpc>`, and `impl<V: Serialize> IntoReply<Rpc> for V` is refused
//! by the orphan rule (E0210, an uncovered `V` before the local `Rpc`) before any overlap with
//! `()` or a stream could be (E0119).

use std::any::TypeId;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use bytes::Bytes;
use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use serde::Serialize;
use ulo::{BoxError, BoxFuture, Shape};
use ulo_transport::{CallError, ErrorKind, IntoReply, IntoReplyError, Tracked};

pub use ulo_transport as transport;
pub use ulo_transport::__private::{Param, ViaCall, ViaContainer, controller};

use crate::extract::{Inbound, Payload};
use crate::frame::{Data, PayloadKind};
use crate::transport::{Reply, Rpc, RpcCx};

/// A handler's call: extraction, the controller, the handler and the reply probe, run by
/// `dispatch` after every guard admits.
pub type HandlerFn = Arc<dyn Fn(RpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// Whether a handler answers calls or takes events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `#[message]`: request-reply, in any of the four shapes.
    Message,
    /// `#[event]`: fire-and-forget.
    Event,
}

/// The handler value `#[message]` and `#[event]` pass to `HandlerSpec::new`, read back from
/// `MountedHandler::handler` by `Server::prepare`: the pattern, the kind, the call, and the payload
/// kind its parameters read.
pub struct RpcHandler {
    pub(crate) pattern: &'static str,
    pub(crate) kind: Kind,
    pub(crate) call: HandlerFn,
    pub(crate) payload: PayloadKind,
}

/// The shape a handler's signature declares: `streams_request` from the `Inbound<T>` probe over
/// its parameters, `streams_reply` from the [`ReplyShapeProbe`] over its output.
pub fn shape(streams_request: bool, streams_reply: bool) -> Shape {
    match (streams_request, streams_reply) {
        (false, false) => Shape::Unary,
        (false, true) => Shape::ServerStreaming,
        (true, false) => Shape::ClientStreaming,
        (true, true) => Shape::Bidi,
    }
}

/// `(&&InboundProbe::<P>::new()).streams()`: `true` for an `Inbound<T>` parameter, `false` for any
/// other, ranked by autoref at the concrete parameter type, so an alias of `Inbound` counts.
pub struct InboundProbe<P>(PhantomData<fn() -> P>);

impl<P> InboundProbe<P> {
    pub fn new() -> Self {
        InboundProbe(PhantomData)
    }
}

impl<P> Default for InboundProbe<P> {
    fn default() -> Self {
        InboundProbe::new()
    }
}

pub trait ViaInbound {
    fn streams(&self) -> bool;
}

impl<T> ViaInbound for &InboundProbe<Inbound<T>> {
    fn streams(&self) -> bool {
        true
    }
}

pub trait NotInbound {
    fn streams(&self) -> bool;
}

impl<P> NotInbound for InboundProbe<P> {
    fn streams(&self) -> bool {
        false
    }
}

/// `(&&&ReplyShapeProbe::of(&|| async { .. })).streams()`: whether a handler's output streams its
/// reply, read off the type the handler returns rather than its spelling, so a stream behind a
/// type alias and an opaque `impl Stream` count alike.
///
/// The attribute writes, in the mount function, a closure that is never called, whose future
/// calls the handler with the receiver and each parameter bound to `unreachable!()`:
///
/// ```text
/// let __ulo_reply = ReplyShapeProbe::of(&|| async move {
///     let __ulo_this: &Self = unreachable!();
///     let __ulo_arg0: Payload<Order> = unreachable!();
///     Self::create(__ulo_this, __ulo_arg0).await
/// });
/// ```
///
/// [`of`](Self::of) names the future's output `R`, and three arms ranked by autoref read it: a
/// `Result` whose `Ok` side is a stream on `&&ReplyShapeProbe`, a stream on `&ReplyShapeProbe`,
/// any other output on `ReplyShapeProbe`. `#[event]` reads the same probe through `event_reply`,
/// whose two stream arms answer [`StreamedReplyOnEvent`], refused by `let (): ()` with E0308 at
/// the return type.
pub struct ReplyShapeProbe<R>(PhantomData<fn() -> R>);

impl<R> ReplyShapeProbe<R> {
    pub fn of<F, Fut>(_call: &F) -> Self
    where
        F: Fn() -> Fut,
        Fut: Future<Output = R>,
    {
        ReplyShapeProbe(PhantomData)
    }
}

/// What `#[event]`'s probe answers for a handler whose output is a stream: an event is answered by
/// nothing, and a streamed reply is `#[message]`'s.
pub struct StreamedReplyOnEvent {
    _private: (),
}

pub trait StreamsInResult {
    fn streams(&self) -> bool;
    fn event_reply(&self) -> StreamedReplyOnEvent;
}

impl<S: Stream, E> StreamsInResult for &&ReplyShapeProbe<Result<S, E>> {
    fn streams(&self) -> bool {
        true
    }

    fn event_reply(&self) -> StreamedReplyOnEvent {
        StreamedReplyOnEvent { _private: () }
    }
}

pub trait StreamsBare {
    fn streams(&self) -> bool;
    fn event_reply(&self) -> StreamedReplyOnEvent;
}

impl<S: Stream> StreamsBare for &ReplyShapeProbe<S> {
    fn streams(&self) -> bool {
        true
    }

    fn event_reply(&self) -> StreamedReplyOnEvent {
        StreamedReplyOnEvent { _private: () }
    }
}

pub trait StreamsNot {
    fn streams(&self) -> bool;
    fn event_reply(&self);
}

impl<R> StreamsNot for ReplyShapeProbe<R> {
    fn streams(&self) -> bool {
        false
    }

    fn event_reply(&self) {}
}

/// The reply probe the generated call names:
/// `(&&&IntoReplyProbe::<Rpc, _>::new(out)).into_reply(&cx)`, with `ViaCallError`, `ViaBoxError`
/// and `ViaValue` imported anonymously.
///
/// The three arms rank the error side by autoref as `ulo-transport`'s do: `Result<V, E>` with
/// `E: Into<CallError>` on `&&IntoReplyProbe`, `Result<V, E>` with `E: Into<BoxError>` on
/// `&IntoReplyProbe`, any answer on `IntoReplyProbe`. The value side is an [`Answer`], whose
/// marker `M` the call infers from the one impl that applies, as `Param<T, M>` does for a
/// parameter: a reply already built or a user's own `IntoReply<Rpc>`, a stream of `Result` items,
/// or any `Serialize` value.
pub struct IntoReplyProbe<T, V> {
    value: Cell<Option<V>>,
    _t: PhantomData<fn() -> T>,
}

impl<T, V> IntoReplyProbe<T, V> {
    pub fn new(value: V) -> Self {
        IntoReplyProbe { value: Cell::new(Some(value)), _t: PhantomData }
    }

    /// The value, once. The generated code converts each probe once; a second call is answered
    /// as an internal error rather than a panic.
    fn take(&self) -> Result<V, BoxError> {
        self.value.take().ok_or_else(|| BoxError::from(CallError::new(ErrorKind::Internal, "internal error")))
    }
}

pub trait ViaCallError<M> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError>;
}

pub trait ViaBoxError<M> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError>;
}

pub trait ViaValue<M> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError>;
}

impl<V: Answer<M>, E: Into<CallError>, M> ViaCallError<M> for &&IntoReplyProbe<Rpc, Result<V, E>> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        }
    }
}

impl<V: Answer<M>, E: Into<BoxError>, M> ViaBoxError<M> for &IntoReplyProbe<Rpc, Result<V, E>> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => Err(err.into()),
        }
    }
}

impl<V: Answer<M>, M> ViaValue<M> for IntoReplyProbe<Rpc, V> {
    fn into_reply(&self, cx: &RpcCx) -> Result<Reply, BoxError> {
        self.take()?.answer(cx)
    }
}

/// A handler's answer once its `Result`, if any, is unwrapped. The markers are disjoint at every
/// type that implements one impl only; a type implementing two, a user's `IntoReply<Rpc>` type that
/// is also `Serialize`, or a named stream type that is also `Serialize`, is ambiguous and fails
/// to compile asking for a type annotation. `()` takes `AsData` and is answered as no payload.
pub trait Answer<M>: Send + 'static {
    fn answer(self, cx: &RpcCx) -> Result<Reply, BoxError>;
}

/// The marker of a reply already built: `Reply`, `Data`, or a user's own `IntoReply<Rpc>`.
pub enum AsReply {}

/// The marker of a `Serialize` value, encoded by the link's codec as one `res`.
pub enum AsData {}

/// The marker of a stream of `Result` items, each `Ok` written as `item`, its error side ranked
/// by [`ItemError`]'s marker `M`.
pub struct AsStream<M>(PhantomData<M>);

impl<V: IntoReply<Rpc>> Answer<AsReply> for V {
    fn answer(self, cx: &RpcCx) -> Result<Reply, BoxError> {
        <V as IntoReply<Rpc>>::into_reply(self, cx).map_err(|err| BoxError::from(CallError::from(err)))
    }
}

impl<V: Serialize + Send + 'static> Answer<AsData> for V {
    fn answer(self, cx: &RpcCx) -> Result<Reply, BoxError> {
        // `()` is `Serialize`, so it cannot have an `AsReply` impl of its own without making
        // every `()` answer ambiguous; it is told apart here instead.
        if TypeId::of::<V>() == TypeId::of::<()>() {
            return Ok(Reply::None);
        }
        cx.codec().encode(&self).map(Reply::One).map_err(encoding_failed)
    }
}

impl<S, U, E, M> Answer<AsStream<M>> for S
where
    S: Stream<Item = Result<U, E>> + Send + 'static,
    U: Serialize + Send + 'static,
    E: ItemError<M>,
    M: 'static,
{
    fn answer(self, cx: &RpcCx) -> Result<Reply, BoxError> {
        let codec = cx.codec();
        let items: BoxStream<'static, Result<Data, BoxError>> = Box::pin(self.map(move |item| match item {
            Ok(value) => codec.encode(&value).map_err(encoding_failed),
            Err(err) => Err(err.into_boxed()),
        }));
        Ok(Reply::Many(Tracked::new(items, cx.exec().clone())))
    }
}

/// A stream item's error side: anything `Into<CallError>`, so a domain error keeps its kind on the
/// late path, or a `BoxError` as it stands. The two are disjoint, a `BoxError` being no
/// `Classify` error.
pub trait ItemError<M>: Send + 'static {
    fn into_boxed(self) -> BoxError;
}

pub enum ItemCall {}

pub enum ItemBoxed {}

impl<E: Into<CallError> + Send + 'static> ItemError<ItemCall> for E {
    fn into_boxed(self) -> BoxError {
        let err: CallError = self.into();
        BoxError::from(err)
    }
}

impl ItemError<ItemBoxed> for BoxError {
    fn into_boxed(self) -> BoxError {
        self
    }
}

/// A value the codec refused reaches the error handlers as a `CallError` of kind `Internal`
/// holding the `IntoReplyError`, as `ulo-transport`'s probe reports one.
fn encoding_failed(err: BoxError) -> BoxError {
    BoxError::from(CallError::from(IntoReplyError::new(err)))
}

impl RpcHandler {
    pub fn new<F>(pattern: &'static str, kind: Kind, call: F) -> Self
    where
        F: Fn(RpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync + 'static,
    {
        RpcHandler { pattern, kind, call: Arc::new(call), payload: PayloadKind::Serde }
    }

    /// One parameter's payload kind, from `PayloadKindProbe`: `Some` for a payload parameter.
    pub fn payload(mut self, kind: Option<PayloadKind>) -> Self {
        if let Some(kind) = kind {
            self.payload = kind;
        }
        self
    }
}

/// `(&&&PayloadKindProbe::<P>::new()).kind()`: `Some(Binary)` for `Payload<Bytes>` and `Bytes`,
/// `Some(Serde)` for any other `Payload<T>`, `None` for a parameter that is not a payload, ranked by
/// autoref at the concrete parameter type.
pub struct PayloadKindProbe<P>(PhantomData<fn() -> P>);

impl<P> PayloadKindProbe<P> {
    pub fn new() -> Self {
        PayloadKindProbe(PhantomData)
    }
}

impl<P> Default for PayloadKindProbe<P> {
    fn default() -> Self {
        PayloadKindProbe::new()
    }
}

pub trait ViaBinary {
    fn kind(&self) -> Option<PayloadKind>;
}

impl ViaBinary for &&PayloadKindProbe<Payload<Bytes>> {
    fn kind(&self) -> Option<PayloadKind> {
        Some(PayloadKind::Binary)
    }
}

impl ViaBinary for &&PayloadKindProbe<Bytes> {
    fn kind(&self) -> Option<PayloadKind> {
        Some(PayloadKind::Binary)
    }
}

pub trait ViaSerde {
    fn kind(&self) -> Option<PayloadKind>;
}

impl<T> ViaSerde for &PayloadKindProbe<Payload<T>> {
    fn kind(&self) -> Option<PayloadKind> {
        Some(PayloadKind::Serde)
    }
}

pub trait NotPayload {
    fn kind(&self) -> Option<PayloadKind>;
}

impl<P> NotPayload for PayloadKindProbe<P> {
    fn kind(&self) -> Option<PayloadKind> {
        None
    }
}
