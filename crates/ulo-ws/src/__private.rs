//! Support for the code `ulo-ws-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::marker::PhantomData;
use std::sync::Arc;

use ulo::{BoxError, BoxFuture, Mount};

pub use ulo_transport as transport;

use crate::connection::Connection;
use crate::gateway::{AfterInit, ConnectRefused, DisconnectReason, GatewayConfig, GatewayRef, OnConnect, OnDisconnect};
use crate::transport::{ConnectCx, Reply, WsCx};

/// A message handler's call: extraction, the controller, the handler and the reply probe, run by
/// `dispatch` after every guard admits.
pub type HandlerFn = Arc<dyn Fn(WsCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// The handler value `#[message]` passes to `HandlerSpec::new`, read back from
/// `MountedHandler::handler` by the hand-off and the standalone server: the event, the call, and
/// whether a parameter reads the payload as `Payload<T>`.
pub struct WsHandler {
    pub(crate) event: &'static str,
    pub(crate) call: HandlerFn,
    pub(crate) reads_payload: bool,
}

impl WsHandler {
    pub fn new<F>(event: &'static str, call: F) -> Self
    where
        F: Fn(WsCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync + 'static,
    {
        WsHandler { event, call: Arc::new(call), reads_payload: false }
    }

    /// One parameter's payload read, from `PayloadProbe`.
    pub fn payload(mut self, reads: bool) -> Self {
        self.reads_payload |= reads;
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
    let _ = (m, hooks);
    todo!()
}
