//! The RPC client (transports DESIGN §5.4): requests, events and the streamed shapes over a link's
//! client side, connected lazily on the first call.
//!
//! ```ignore
//! self.billing.request::<_, Invoice>("invoices.create", &NewInvoice::from(order))
//!     .header("tenant", order.tenant())
//!     .timeout(Duration::from_secs(2))
//!     .await
//! ```

use std::error::Error;
use std::fmt;
use std::future::IntoFuture;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use futures_core::stream::BoxStream;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::{BoxFuture, ExecutionRef};
use ulo_transport::{Classify, Details, ErrorKind};

/// A client of one link, bound by `RpcClientModule`: `Dep<RpcClient, Billing>` under the module's
/// qualifier. Cheap to clone. Every timeout runs on the app's `Timer`; dropping a pending request
/// or a stream sends `cancel`. There is no ambient execution: a call made inside one forwards its
/// deadline and cancellation with [`Call::within`].
#[derive(Clone)]
pub struct RpcClient {
    pub(crate) inner: Arc<ClientInner>,
}

pub(crate) struct ClientInner {}

impl RpcClient {
    /// A request answered by one reply.
    pub fn request<Req, Res>(&self, pattern: &str, req: &Req) -> Call<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let _ = (pattern, req);
        todo!()
    }

    /// An event, answered by nothing.
    pub fn emit<T: Serialize + ?Sized>(&self, pattern: &str, data: &T) -> Emit {
        let _ = (pattern, data);
        todo!()
    }

    /// A request answered by a stream of replies.
    pub fn stream<Req, Res>(&self, pattern: &str, req: &Req) -> StreamCall<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let _ = (pattern, req);
        todo!()
    }

    /// A streamed request answered by one reply.
    pub fn send_stream<S, Res>(&self, pattern: &str, items: S) -> Call<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        let _ = (pattern, items);
        todo!()
    }

    /// A streamed request answered by a stream of replies.
    pub fn duplex<S, Res>(&self, pattern: &str, items: S) -> StreamCall<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        let _ = (pattern, items);
        todo!()
    }
}

/// A call answered by one reply, sent when awaited.
#[must_use = "a call is sent when awaited"]
pub struct Call<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
}

impl<Res> Call<Res> {
    /// A header on the call, `h` or the broker's own headers.
    pub fn header(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let _ = (name, value);
        todo!()
    }

    /// This call's timeout in place of the module's.
    pub fn timeout(self, timeout: Duration) -> Self {
        let _ = timeout;
        todo!()
    }

    /// Forwards `exec`'s remaining deadline as `deadline-ms` and cancels the call when `exec` is
    /// cancelled.
    pub fn within(self, exec: &ExecutionRef) -> Self {
        let _ = exec;
        todo!()
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for Call<Res> {
    type Output = Result<Res, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<Res, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        todo!()
    }
}

/// An event, published when awaited.
#[must_use = "an event is published when awaited"]
pub struct Emit {
    pub(crate) _private: (),
}

impl Emit {
    pub fn header(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let _ = (name, value);
        todo!()
    }

    pub fn within(self, exec: &ExecutionRef) -> Self {
        let _ = exec;
        todo!()
    }
}

impl IntoFuture for Emit {
    type Output = Result<(), RpcError>;
    type IntoFuture = BoxFuture<'static, Result<(), RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        todo!()
    }
}

/// A call answered by a stream, opened when awaited.
#[must_use = "a call is sent when awaited"]
pub struct StreamCall<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
}

impl<Res> StreamCall<Res> {
    pub fn header(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let _ = (name, value);
        todo!()
    }

    /// The longest wait for each reply frame, the module's timeout unset.
    pub fn timeout(self, timeout: Duration) -> Self {
        let _ = timeout;
        todo!()
    }

    pub fn within(self, exec: &ExecutionRef) -> Self {
        let _ = exec;
        todo!()
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for StreamCall<Res> {
    type Output = Result<RpcStream<Res>, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<RpcStream<Res>, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        todo!()
    }
}

/// A stream of replies: `item` frames decoded as `Res`, ending at `end`, or with one `Err` at an
/// `err`. Dropped before its end, it sends `cancel`.
pub struct RpcStream<Res> {
    pub(crate) items: BoxStream<'static, Result<Res, RpcError>>,
}

impl<Res> Stream for RpcStream<Res> {
    type Item = Result<Res, RpcError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}

/// What a caller sees, uniform across links (transports DESIGN §5.2): the server's `err` with its
/// kind and details, or the link's own failure mapped one way everywhere. No responders, a lost
/// link or a broker refusal is `Unavailable`; the client's timeout `Timeout`; an oversized payload
/// `BadRequest` with `reason: "payload_too_large"`; a binary payload on a JSON link `BadRequest`
/// with `reason: "binary_unsupported"`, before any I/O. A pattern nothing handles is `Unavailable`
/// with `reason: "pattern_unhandled"` or `"no_destination"` where the link signals it, and the
/// client's `Timeout` where it cannot.
#[derive(Debug)]
pub struct RpcError {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) details: Details,
}

impl RpcError {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &Details {
        &self.details
    }

    /// The `ErrorInfo` detail's `reason`, when one is present: `"no_destination"`,
    /// `"pattern_unhandled"`, `"payload_too_large"`, `"binary_unsupported"`.
    pub fn reason(&self) -> Option<&str> {
        todo!()
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl Error for RpcError {}

impl Classify for RpcError {
    fn classify(&self) -> ErrorKind {
        self.kind
    }

    fn details(&self) -> Details {
        self.details.clone()
    }
}
