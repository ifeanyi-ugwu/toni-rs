//! Support for the code `ulo-rpc-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::marker::PhantomData;
use std::sync::Arc;

use bytes::Bytes;
use ulo::{BoxError, BoxFuture};

pub use ulo_transport as transport;

use crate::extract::Payload;
use crate::frame::PayloadKind;
use crate::transport::{Reply, RpcCx};

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
