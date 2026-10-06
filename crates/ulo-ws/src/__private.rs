//! Support for the code `ulo-ws-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::any::TypeId;
use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use serde::Serialize;
use ulo::{BoxError, BoxFuture, Mount, Transport};
use ulo_transport::{CallError, ErrorKind, IntoReply, Tracked};

pub use ulo_transport as transport;

use crate::connection::Connection;
use crate::gateway::{
    AfterInit, ConnectHandler, ConnectRefused, DisconnectReason, Gateway, GatewayConfig, GatewayRef, OnConnect, OnDisconnect,
};
use crate::transport::{ConnectCx, Reply, Ws, WsCx};

/// A message handler's call: extraction, the controller, the handler and the reply probe, run by
/// `dispatch` after every guard admits.
pub type HandlerFn = Arc<dyn Fn(WsCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// The handler value `#[message]` passes to `HandlerSpec::new`, read back from
/// `MountedHandler::handler` by the hand-off and the standalone server: the event and the call.
pub struct WsHandler {
    pub(crate) event: &'static str,
    pub(crate) call: HandlerFn,
}

impl WsHandler {
    pub fn new<F>(event: &'static str, call: F) -> Self
    where
        F: Fn(WsCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync + 'static,
    {
        WsHandler { event, call: Arc::new(call) }
    }

    /// Accepts a parameter's `PayloadProbe` answer and records nothing: a message's `data` stays
    /// undecoded until a `Payload<T>` reads it, so no handler needs it marked.
    pub fn payload(self, reads: bool) -> Self {
        let _ = reads;
        self
    }
}

/// `(&&PayloadProbe::<P>::new()).reads()`: `true` for a `Payload<T>` parameter, `false` for any
/// other, ranked by autoref at the concrete parameter type.
pub struct PayloadProbe<P>(PhantomData<fn() -> P>);

impl<P> PayloadProbe<P> {
    pub fn new() -> Self {
        PayloadProbe(PhantomData)
    }
}

impl<P> Default for PayloadProbe<P> {
    fn default() -> Self {
        PayloadProbe::new()
    }
}

pub trait ViaPayload {
    fn reads(&self) -> bool;
}

impl<T> ViaPayload for &PayloadProbe<crate::envelope::Payload<T>> {
    fn reads(&self) -> bool {
        true
    }
}

pub trait NotPayload {
    fn reads(&self) -> bool;
}

impl<P> NotPayload for PayloadProbe<P> {
    fn reads(&self) -> bool {
        false
    }
}

/// `OnConnect::on_connect` as the connect handler calls it.
pub type OnConnectFn<G> = for<'a> fn(&'a G, &'a ConnectCx) -> BoxFuture<'a, Result<(), ConnectRefused>>;

/// `OnDisconnect::on_disconnect` as the connection's terminal execution calls it.
pub type OnDisconnectFn<G> = for<'a> fn(&'a G, &'a Connection, DisconnectReason) -> BoxFuture<'a, ()>;

/// `AfterInit::after_init` as the hand-off's `bound` or the standalone server calls it.
pub type AfterInitFn<G> = for<'a> fn(&'a G, GatewayRef) -> BoxFuture<'a, ()>;

/// The connection hooks gateway `G` implements, found by the attribute's probes at the concrete
/// type, since a generic default cannot see the impls.
pub struct Hooks<G> {
    pub(crate) on_connect: Option<OnConnectFn<G>>,
    pub(crate) on_disconnect: Option<OnDisconnectFn<G>>,
    pub(crate) after_init: Option<AfterInitFn<G>>,
}

impl<G> Hooks<G> {
    pub fn new() -> Self {
        Hooks { on_connect: None, on_disconnect: None, after_init: None }
    }

    pub fn on_connect(mut self, hook: Option<OnConnectFn<G>>) -> Self {
        self.on_connect = hook;
        self
    }

    pub fn on_disconnect(mut self, hook: Option<OnDisconnectFn<G>>) -> Self {
        self.on_disconnect = hook;
        self
    }

    pub fn after_init(mut self, hook: Option<AfterInitFn<G>>) -> Self {
        self.after_init = hook;
        self
    }
}

impl<G> Default for Hooks<G> {
    fn default() -> Self {
        Hooks::new()
    }
}

/// `(&&HookProbe::<Self>::new()).on_connect()` and its two siblings: `Some` where the gateway type
/// implements the hook's trait, `None` otherwise.
pub struct HookProbe<G>(PhantomData<fn() -> G>);

impl<G> HookProbe<G> {
    pub fn new() -> Self {
        HookProbe(PhantomData)
    }
}

impl<G> Default for HookProbe<G> {
    fn default() -> Self {
        HookProbe::new()
    }
}

pub trait ViaOnConnect<G> {
    fn on_connect(&self) -> Option<OnConnectFn<G>>;
}

impl<G: OnConnect> ViaOnConnect<G> for &HookProbe<G> {
    fn on_connect(&self) -> Option<OnConnectFn<G>> {
        Some(call_on_connect::<G>)
    }
}

pub trait NoOnConnect<G> {
    fn on_connect(&self) -> Option<OnConnectFn<G>>;
}

impl<G> NoOnConnect<G> for HookProbe<G> {
    fn on_connect(&self) -> Option<OnConnectFn<G>> {
        None
    }
}

pub trait ViaOnDisconnect<G> {
    fn on_disconnect(&self) -> Option<OnDisconnectFn<G>>;
}

impl<G: OnDisconnect> ViaOnDisconnect<G> for &HookProbe<G> {
    fn on_disconnect(&self) -> Option<OnDisconnectFn<G>> {
        Some(call_on_disconnect::<G>)
    }
}

pub trait NoOnDisconnect<G> {
    fn on_disconnect(&self) -> Option<OnDisconnectFn<G>>;
}

impl<G> NoOnDisconnect<G> for HookProbe<G> {
    fn on_disconnect(&self) -> Option<OnDisconnectFn<G>> {
        None
    }
}

pub trait ViaAfterInit<G> {
    fn after_init(&self) -> Option<AfterInitFn<G>>;
}

impl<G: AfterInit> ViaAfterInit<G> for &HookProbe<G> {
    fn after_init(&self) -> Option<AfterInitFn<G>> {
        Some(call_after_init::<G>)
    }
}

pub trait NoAfterInit<G> {
    fn after_init(&self) -> Option<AfterInitFn<G>>;
}

impl<G> NoAfterInit<G> for HookProbe<G> {
    fn after_init(&self) -> Option<AfterInitFn<G>> {
        None
    }
}

fn call_on_connect<'a, G: OnConnect>(gateway: &'a G, cx: &'a ConnectCx) -> BoxFuture<'a, Result<(), ConnectRefused>> {
    Box::pin(gateway.on_connect(cx))
}

fn call_on_disconnect<'a, G: OnDisconnect>(gateway: &'a G, conn: &'a Connection, why: DisconnectReason) -> BoxFuture<'a, ()> {
    Box::pin(gateway.on_disconnect(conn, why))
}

fn call_after_init<G: AfterInit>(gateway: &G, gw: GatewayRef) -> BoxFuture<'_, ()> {
    Box::pin(gateway.after_init(gw))
}

/// What `GatewayConfig::mount_gateway` writes for an attributed gateway: the connect handler
/// under `WsConnect`, carrying the gateway's settings, its connect guards, its session factory and
/// the hooks it implements. Called inside `Mount::once::<G>`.
pub fn mount_connect<G: GatewayConfig>(m: &mut Mount<'_>, hooks: Hooks<G>) {
    crate::gateway::mount::<G>(m, ConnectHandler::attributed(hooks));
}

/// The `Mount::once` key of a hand-written gateway's connect handler, apart from the gateway
/// type's own key, which a `#[message]` mount function spends on `mount_gateway`.
pub struct HandWritten<G>(PhantomData<fn() -> G>);

/// What `Gateway::mount` runs once per controller type.
pub fn mount_hand_written<G: Gateway>(m: &mut Mount<'_>) {
    crate::gateway::mount::<G>(m, ConnectHandler::hand_written::<G>());
}

/// The paths `#[message]` gives `ulo-handler-codegen` for its generated call: `Param` and
/// `controller` as `ulo-transport` writes them, and the WebSocket reply probe in place of
/// `ulo-transport`'s, which reaches only `IntoReply` types.
pub mod reply {
    pub mod __private {
        pub use crate::__private::{IntoReplyProbe, ViaBoxError, ViaCallError, ViaValue};
        pub use ulo_transport::__private::{Param, ViaCall, ViaContainer, controller};
    }
}

/// The value-side marker of an [`Answer`] that is an `IntoReply<Ws>` type: `Frame`, `Reply`.
pub enum ViaReply {}
/// The value-side marker of an [`Answer`] that is a stream of `Result<T: Serialize, E>`.
pub enum ViaStream {}
/// The value-side marker of an [`Answer`] that is a `T: Serialize`, `()` included.
pub enum ViaSerde {}

/// What a message handler may answer with, besides the `Result` around it (transports DESIGN
/// §4.1): `()`, any `T: Serialize`, a `Frame` or `Reply`, and a stream of `Result<T, E>` with
/// `T: Serialize` and `E: Into<CallError>`.
///
/// `IntoReply<Ws>` cannot carry the serializable values and the streams itself: a blanket
/// `impl<T: Serialize> IntoReply<Ws> for T` is refused by the orphan rule, `T` being an uncovered
/// type parameter ahead of the local `Ws` (E0210), and the stream and serde blankets would
/// overlap besides (E0119). The marker `M` keeps the three blankets apart as three traits, and the
/// reply probe infers it from the one whose where-clauses the concrete value meets, as `Param<T, M>`
/// infers `ViaCall` or `ViaContainer`. A type that is both a stream and serializable is ambiguous
/// at the handler, a compile error there.
pub trait Answer<M>: Send + 'static {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError>;
}

impl<V: IntoReply<Ws>> Answer<ViaReply> for V {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        self.into_reply(cx).map_err(|err| BoxError::from(CallError::from(err)))
    }
}

/// Each item encoded by the gateway's codec; an `Err` item becomes the late path's error, and an
/// item that does not encode an internal one.
impl<S, T, E> Answer<ViaStream> for S
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: Serialize + Send + 'static,
    E: Into<CallError> + Send + 'static,
{
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        let codec = cx.conn().inner.gateway.settings().codec;
        let frames = self.map(move |item| match item {
            Ok(value) => codec.encode(&value).map_err(|err| BoxError::from(encode_failure(err))),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        });
        let frames: BoxStream<'static, Result<crate::envelope::Frame, BoxError>> = Box::pin(frames);
        Ok(Reply::Many(Tracked::new(frames, cx.exec().clone())))
    }
}

/// `()` is no answer, `Reply::None`; any other value is encoded by the gateway's codec.
impl<T: Serialize + Send + 'static> Answer<ViaSerde> for T {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        if TypeId::of::<T>() == TypeId::of::<()>() {
            return Ok(Reply::None);
        }
        let codec = cx.conn().inner.gateway.settings().codec;
        codec.encode(&self).map(Reply::One).map_err(|err| BoxError::from(encode_failure(err)))
    }
}

fn encode_failure(err: BoxError) -> CallError {
    CallError::new(ErrorKind::Internal, "internal error").with_source(err)
}

/// The WebSocket reply probe, the shape of `ulo-transport`'s with [`Answer`] on the value side:
/// `(&&&IntoReplyProbe::<Ws, _>::new(out)).into_reply(&cx)`. Three arms, each one reference
/// deeper than the priority reads: `Result<V, E: Into<CallError>>`, `Result<V, E: Into<BoxError>>`,
/// then any `V`. Each arm's trait carries the value marker `M`, inferred from `V: Answer<M>`.
pub struct IntoReplyProbe<T, V> {
    value: Cell<Option<V>>,
    _t: PhantomData<fn() -> T>,
}

impl<T, V> IntoReplyProbe<T, V> {
    pub fn new(value: V) -> Self {
        IntoReplyProbe { value: Cell::new(Some(value)), _t: PhantomData }
    }

    /// The value, once; a second call is answered as an internal error rather than a panic.
    fn take(&self) -> Result<V, BoxError> {
        self.value.take().ok_or_else(|| BoxError::from(CallError::new(ErrorKind::Internal, "internal error")))
    }
}

pub trait ViaCallError<M> {
    fn into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

pub trait ViaBoxError<M> {
    fn into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

pub trait ViaValue<M> {
    fn into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

impl<V: Answer<M>, E: Into<CallError>, M> ViaCallError<M> for &&IntoReplyProbe<Ws, Result<V, E>> {
    fn into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        }
    }
}

impl<V: Answer<M>, E: Into<BoxError>, M> ViaBoxError<M> for &IntoReplyProbe<Ws, Result<V, E>> {
    fn into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => Err(err.into()),
        }
    }
}

impl<V: Answer<M>, M> ViaValue<M> for IntoReplyProbe<Ws, V> {
    fn into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        self.take()?.answer(cx)
    }
}
