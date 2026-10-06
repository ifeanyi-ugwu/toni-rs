//! Support for the code `ulo-grpc-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.
//!
//! `#[method(Marker)]` writes, beside the handler, a hidden method taking the same parameters that
//! calls the handler and turns its output into an [`Answer`] through [`ReplyProbe`]. The
//! `ulo-handler-codegen` call then reads that hidden method, whose `Result<Answer, BoxError>` is
//! an `IntoReply<Grpc>` answer. The detour exists because a handler answers a prost message or a
//! stream of them, and neither can implement `IntoReply<Grpc>`: a blanket impl in this crate over a
//! foreign trait is refused by the orphan rule, and a message `ulo-build` wrote cannot cover `()`
//! or a `prost_types` message.
//!
//! The hidden method also carries the compile-time checks of the handler against its marker: each
//! body-consuming parameter's message type and shape through [`ParamProbe`], and the reply's
//! message type and shape through [`ReplyProbe`]. A mismatch leaves [`ParamCheck::matches`] or
//! [`Checked::checked`] uncallable, an E0599 whose type spells what the handler declares beside
//! what the marker requires: `Checked<(Shaped<true>, UserEvent), (Shaped<false>, User)>` is a
//! stream of `UserEvent` returned for a unary method replying `User`.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use futures_core::Stream;
use tonic::metadata::MetadataMap;
use ulo::{BoxError, BoxFuture, Shape};
use ulo_transport::{CallError, ErrorKind, IntoReply, IntoReplyError};

pub use ulo_transport as transport;

use crate::dispatch::{encode_one, encode_stream};
use crate::extract::{Message, Request, Streaming};
use crate::method::Method;
use crate::transport::{Grpc, GrpcCx, Reply, Response};

/// A handler's call: extraction, the controller, the handler and the reply probe, run by
/// `dispatch` after every guard admits.
pub type HandlerFn = Arc<dyn Fn(GrpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// The handler value `#[method(Marker)]` passes to `HandlerSpec::new`, read back from
/// `MountedHandler::handler` by the server: the marker's path, and the call.
pub struct GrpcHandler {
    pub(crate) path: &'static str,
    pub(crate) call: HandlerFn,
}

impl GrpcHandler {
    /// The handler for marker `M`.
    pub fn new<M, F>(call: F) -> Self
    where
        M: Method,
        F: Fn(GrpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync + 'static,
    {
        GrpcHandler { path: M::PATH, call: Arc::new(call) }
    }
}

/// The compile-time check of a handler against its marker: the attribute names the handler's
/// message type and reply type at the concrete site, and the bounds fail where they differ.
pub fn check_types<M, Req, Res>()
where
    M: Method<Request = Req, Response = Res>,
{
}

/// Whether a method of shape `shape` streams its request: the const argument of every
/// [`ParamProbe`] check.
pub const fn streams_request(shape: Shape) -> bool {
    matches!(shape, Shape::ClientStreaming | Shape::Bidi)
}

/// Whether a method of shape `shape` streams its reply: the const argument of the
/// [`ReplyProbe`] check.
pub const fn streams_reply(shape: Shape) -> bool {
    matches!(shape, Shape::ServerStreaming | Shape::Bidi)
}

/// One side of a call, one message or a stream of them, as a type the checks compare.
pub struct Shaped<const STREAMED: bool>;

/// What one parameter declares beside what the marker requires; [`matches`](Self::matches)
/// exists only where the two are one type.
pub struct ParamCheck<Got, Want>(PhantomData<fn() -> (Got, Want)>);

impl<T> ParamCheck<T, T> {
    pub fn matches(self) {}
}

/// `(&&ParamProbe::<M, P>::new()).body::<{ streams_request(M::SHAPE) }>().matches()`, written once
/// per parameter: `Message<T>` and `Request<T>` declare one message `T`, `Streaming<T>` a stream of
/// `T`, each compared with `M::Request` and `M`'s request shape; any other parameter compares `()`
/// with `()`. Ranked by autoref at the concrete parameter type, so an alias of `Message` is
/// checked as one.
pub struct ParamProbe<M, P>(PhantomData<fn() -> (M, P)>);

impl<M, P> ParamProbe<M, P> {
    pub fn new() -> Self {
        ParamProbe(PhantomData)
    }
}

impl<M, P> Default for ParamProbe<M, P> {
    fn default() -> Self {
        ParamProbe::new()
    }
}

pub trait ViaBody<M: Method> {
    type Got;
    fn body<const STREAMED: bool>(&self) -> ParamCheck<Self::Got, (Shaped<STREAMED>, M::Request)>;
}

impl<M: Method, T> ViaBody<M> for &ParamProbe<M, Message<T>> {
    type Got = (Shaped<false>, T);

    fn body<const STREAMED: bool>(&self) -> ParamCheck<Self::Got, (Shaped<STREAMED>, M::Request)> {
        ParamCheck(PhantomData)
    }
}

impl<M: Method, T> ViaBody<M> for &ParamProbe<M, Request<T>> {
    type Got = (Shaped<false>, T);

    fn body<const STREAMED: bool>(&self) -> ParamCheck<Self::Got, (Shaped<STREAMED>, M::Request)> {
        ParamCheck(PhantomData)
    }
}

impl<M: Method, T> ViaBody<M> for &ParamProbe<M, Streaming<T>> {
    type Got = (Shaped<true>, T);

    fn body<const STREAMED: bool>(&self) -> ParamCheck<Self::Got, (Shaped<STREAMED>, M::Request)> {
        ParamCheck(PhantomData)
    }
}

pub trait NotBody {
    fn body<const STREAMED: bool>(&self) -> ParamCheck<(), ()>;
}

impl<M, P> NotBody for ParamProbe<M, P> {
    fn body<const STREAMED: bool>(&self) -> ParamCheck<(), ()> {
        ParamCheck(PhantomData)
    }
}

/// The handler's output turned into an answer, with what it declares beside what the marker
/// requires; [`checked`](Self::checked) exists only where the two are one type.
pub struct Checked<Got, Want> {
    answer: Result<Answer, BoxError>,
    _types: PhantomData<fn() -> (Got, Want)>,
}

impl<Got, Want> Checked<Got, Want> {
    fn new(answer: Result<Answer, BoxError>) -> Self {
        Checked { answer, _types: PhantomData }
    }
}

impl<T> Checked<T, T> {
    pub fn checked(self) -> Result<Answer, BoxError> {
        self.answer
    }
}

/// The reply probe of a gRPC handler:
/// `(&&&&&&ReplyProbe::<M, _>::new(out)).answer::<{ streams_reply(M::SHAPE) }>().checked()`.
///
/// Six arms, each one reference deeper than the priority reads, the first that applies winning: a
/// `Result` of a [`ReplyStream`] whose error converts into a `CallError`, then one whose error
/// boxes, then a bare [`ReplyStream`]; then the same three over a [`ReplyValue`]. The streams come
/// first, so a type that is both is answered as a stream. An `Err` becomes
/// `BoxError::from(CallError::from(e))` on the first arm of each three and is boxed unchanged on
/// the second, as `ulo-handler-codegen`'s probe does, so a `tonic::Status` returned as the error
/// reaches the wire as it stands. The methods take `&self`, which the ranking needs, so the value
/// sits in a `Cell`.
pub struct ReplyProbe<M, V> {
    value: Cell<Option<V>>,
    _marker: PhantomData<fn() -> M>,
}

impl<M, V> ReplyProbe<M, V> {
    pub fn new(value: V) -> Self {
        ReplyProbe { value: Cell::new(Some(value)), _marker: PhantomData }
    }

    /// The value, once. The generated code converts each probe once; a second call is answered
    /// as an internal error rather than a panic.
    fn take(&self) -> Result<V, BoxError> {
        self.value.take().ok_or_else(|| BoxError::from(CallError::new(ErrorKind::Internal, "internal error")))
    }
}

pub trait StreamViaCallError<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

pub trait StreamViaBoxError<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

pub trait StreamValue<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

pub trait ValueViaCallError<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

pub trait ValueViaBoxError<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

pub trait Value<M: Method> {
    type Got;
    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)>;
}

impl<M: Method, S: ReplyStream, E: Into<CallError>> StreamViaCallError<M> for &&&&&ReplyProbe<M, Result<S, E>> {
    type Got = (Shaped<true>, S::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(match self.take() {
            Ok(Ok(stream)) => Ok(Answer::stream(stream)),
            Ok(Err(err)) => Err(BoxError::from(err.into())),
            Err(err) => Err(err),
        })
    }
}

impl<M: Method, S: ReplyStream, E: Into<BoxError>> StreamViaBoxError<M> for &&&&ReplyProbe<M, Result<S, E>> {
    type Got = (Shaped<true>, S::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(match self.take() {
            Ok(Ok(stream)) => Ok(Answer::stream(stream)),
            Ok(Err(err)) => Err(err.into()),
            Err(err) => Err(err),
        })
    }
}

impl<M: Method, S: ReplyStream> StreamValue<M> for &&&ReplyProbe<M, S> {
    type Got = (Shaped<true>, S::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(self.take().map(Answer::stream))
    }
}

impl<M: Method, V: ReplyValue, E: Into<CallError>> ValueViaCallError<M> for &&ReplyProbe<M, Result<V, E>> {
    type Got = (Shaped<false>, V::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(match self.take() {
            Ok(Ok(value)) => Ok(Answer::value(value)),
            Ok(Err(err)) => Err(BoxError::from(err.into())),
            Err(err) => Err(err),
        })
    }
}

impl<M: Method, V: ReplyValue, E: Into<BoxError>> ValueViaBoxError<M> for &ReplyProbe<M, Result<V, E>> {
    type Got = (Shaped<false>, V::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(match self.take() {
            Ok(Ok(value)) => Ok(Answer::value(value)),
            Ok(Err(err)) => Err(err.into()),
            Err(err) => Err(err),
        })
    }
}

impl<M: Method, V: ReplyValue> Value<M> for ReplyProbe<M, V> {
    type Got = (Shaped<false>, V::Message);

    fn answer<const STREAMED: bool>(&self) -> Checked<Self::Got, (Shaped<STREAMED>, M::Response)> {
        Checked::new(self.take().map(Answer::value))
    }
}

/// A handler's reply of one message: the message, or [`Response`] around it with reply metadata.
/// `()` is one too, being prost's `google.protobuf.Empty`.
pub trait ReplyValue: Send + 'static {
    type Message;
    fn into_grpc(self) -> Reply;
}

impl<T: prost::Message + Default + Send + 'static> ReplyValue for T {
    type Message = T;

    fn into_grpc(self) -> Reply {
        encode_one(self, MetadataMap::new())
    }
}

impl<T: prost::Message + Default + Send + 'static> ReplyValue for Response<T> {
    type Message = T;

    fn into_grpc(self) -> Reply {
        encode_one(self.message, self.metadata)
    }
}

/// A handler's reply of a stream: any stream of [`ReplyItem`]s, or [`Response`] around one with
/// reply metadata.
pub trait ReplyStream: Send + 'static {
    type Message;
    fn into_grpc(self, cx: &GrpcCx) -> Reply;
}

impl<S> ReplyStream for S
where
    S: Stream + Send + 'static,
    S::Item: ReplyItem,
{
    type Message = <S::Item as ReplyItem>::Message;

    fn into_grpc(self, cx: &GrpcCx) -> Reply {
        encode_stream(self, MetadataMap::new(), cx)
    }
}

impl<S> ReplyStream for Response<S>
where
    S: Stream + Send + 'static,
    S::Item: ReplyItem,
{
    type Message = <S::Item as ReplyItem>::Message;

    fn into_grpc(self, cx: &GrpcCx) -> Reply {
        encode_stream(self.message, self.metadata, cx)
    }
}

/// One item of a reply stream: a message, or an error whose kind survives into the error handlers
/// on the late path, since it converts into a `CallError`.
pub trait ReplyItem: Send + 'static {
    type Message: prost::Message + Default + Send + 'static;
    fn into_item(self) -> Result<Self::Message, CallError>;
}

impl<T, E> ReplyItem for Result<T, E>
where
    T: prost::Message + Default + Send + 'static,
    E: Into<CallError> + Send + 'static,
{
    type Message = T;

    fn into_item(self) -> Result<T, CallError> {
        self.map_err(Into::into)
    }
}

/// A handler's output as its reply, built once the call's context is known: a stream needs it for
/// the late path its `Err` items take.
pub struct Answer(Box<dyn FnOnce(&GrpcCx) -> Reply + Send>);

impl Answer {
    fn value<V: ReplyValue>(value: V) -> Answer {
        Answer(Box::new(move |_: &GrpcCx| value.into_grpc()))
    }

    fn stream<S: ReplyStream>(stream: S) -> Answer {
        Answer(Box::new(move |cx: &GrpcCx| stream.into_grpc(cx)))
    }
}

impl IntoReply<Grpc> for Answer {
    fn into_reply(self, cx: &GrpcCx) -> Result<Reply, IntoReplyError> {
        Ok((self.0)(cx))
    }
}
