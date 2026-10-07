use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use futures_core::Stream;
use tonic::Status;
use tonic::metadata::{Ascii, MetadataKey, MetadataMap, MetadataValue};
use ulo::{AppHandle, ExecutionRef, Ext, Extensions, Inputs, LookupError, MountedHandler, Timer, Transport};
use ulo_http::HttpBody;
use ulo_transport::{CallError, ExtractError, FromCall};

use crate::dispatch;

/// The gRPC transport, key `"grpc"`: one execution per call. `#[guards(grpc = ..)]` scopes an entry
/// to its handlers. Its inputs, [`GrpcMetadata`] and [`PeerAddr`], are seeded into every call's
/// execution and declared by the transport itself.
pub struct Grpc;

impl Transport for Grpc {
    const KEY: &'static str = "grpc";

    type Cx = GrpcCx;
    type Reply = Reply;

    fn inputs(d: &mut Inputs) {
        d.input::<GrpcMetadata>().input::<PeerAddr>();
    }
}

impl ulo_http::__private::Carried for Grpc {}

/// gRPC's calls are HTTP/2 requests, so the pre-dispatch stage runs for them as
/// `PreDispatch<Grpc>`, a metadata key apart from HTTP's (X23).
impl ulo_http::HttpCarried for Grpc {}

/// What a handler answers, and what an interceptor's `next.run()` returns: the HTTP response
/// carrying the encoded message or stream. An interceptor reads and writes reply metadata through
/// its headers and cannot read the message, which lets one enhancer list serve every method of a
/// service. A stream item's `Err` runs `dispatch_late` and is written in the trailers. An error
/// handler builds one with [`GrpcCx::reply`], [`GrpcCx::reply_stream`] or
/// [`GrpcCx::reply_status`].
pub type Reply = http::Response<tonic::body::Body>;

/// One call's context: `Clone + Send + Sync`, every clone the same call. The execution, the method
/// path, the request metadata, the peer, and the app's `Timer`.
///
/// A guard reads the metadata through it, `cx.metadata().get("authorization")`, and passes data to
/// the handler through `cx.extensions()`. A streaming reply holds a clone, which keeps the
/// execution's instances and its cancellation signal alive while the stream runs.
#[derive(Clone)]
pub struct GrpcCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    /// The `:path` as the pre-dispatch stage left it.
    pub(crate) path: Arc<str>,
    /// The request headers as the pre-dispatch stage left them; the seeded `GrpcMetadata` input is
    /// what the client sent.
    pub(crate) metadata: GrpcMetadata,
    pub(crate) peer: Option<SocketAddr>,
    /// The matched handler, whose error handlers a stream item's `Err` reaches. `None` in the
    /// context a failure of the unscoped pre-dispatch sub-step is offered with.
    pub(crate) handler: Option<MountedHandler<Grpc>>,
    /// The request body, taken once by the extractor that decodes it.
    pub(crate) body: Mutex<Option<HttpBody>>,
}

impl GrpcCx {
    pub(crate) fn new(inner: CxInner) -> Self {
        GrpcCx { inner: Arc::new(inner) }
    }

    pub fn exec(&self) -> &ExecutionRef {
        &self.inner.exec
    }

    pub fn extensions(&self) -> &Extensions {
        self.inner.exec.extensions()
    }

    /// The extension `T` a guard wrote, as `Ext<T>` reads it.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        self.inner.exec.resolver().ext::<T>()
    }

    /// The path the caller dialled, `/users.v1.UserService/GetUser`.
    pub fn path(&self) -> &str {
        &self.inner.path
    }

    /// The request metadata.
    pub fn metadata(&self) -> &GrpcMetadata {
        &self.inner.metadata
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }

    pub fn app(&self) -> &AppHandle {
        &self.inner.app
    }

    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.inner.timer
    }

    /// One reply message, or [`Response`] around one with reply metadata, encoded as a handler
    /// returning it is answered: what an error handler claiming an error answers with a message of
    /// its own.
    ///
    /// ```ignore
    /// async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
    ///     match err.downcast_ref::<CallError>().and_then(|call| call.source_as::<NotCached>()) {
    ///         Some(_) => Ok(cx.reply(Response::new(pb::User::default()).metadata("x-cache", "miss"))),
    ///         None => Err(err),
    ///     }
    /// }
    /// ```
    pub fn reply<V: ReplyValue>(&self, value: V) -> Reply {
        value.into_grpc()
    }

    /// A reply stream, or [`Response`] around one with reply metadata, as a handler returning it is
    /// answered: each `Ok` item written as a message, an `Err` item run through the matched
    /// handler's error handlers on the late path and written in the trailers, which end the
    /// stream.
    pub fn reply_stream<S: ReplyStream>(&self, stream: S) -> Reply {
        stream.into_grpc(self)
    }

    /// `status` as a trailers-only reply. Answered with `Ok`, it claims the error and ends the
    /// error handlers; returned as the error instead, the next error handler is offered it and the
    /// wire sends it as it stands when none claims it.
    pub fn reply_status(&self, status: Status) -> Reply {
        dispatch::status_reply(status)
    }

    /// The request body, once: the first body-consuming extractor takes it.
    pub(crate) fn take_body(&self) -> Option<HttpBody> {
        self.inner.body.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl AsRef<ExecutionRef> for GrpcCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}

impl FromCall<Grpc> for GrpcCx {
    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        Ok(cx.clone())
    }
}

/// The call's request metadata, as an execution input and as a handler parameter: a token guard
/// reads a bearer token from it.
#[derive(Clone, Debug, Default)]
pub struct GrpcMetadata {
    pub(crate) map: MetadataMap,
}

impl GrpcMetadata {
    pub fn map(&self) -> &MetadataMap {
        &self.map
    }

    /// The ASCII value under `key`, `None` when absent or not ASCII.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).and_then(|value| value.to_str().ok())
    }

    pub(crate) fn from_headers(headers: &http::HeaderMap) -> Self {
        GrpcMetadata { map: MetadataMap::from_headers(headers.clone()) }
    }
}

/// The caller's address, as an execution input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// A reply message with reply metadata: `Response::new(user).metadata("x-cache", "miss")`. Wraps a
/// reply stream the same way, `Response::new(stream)`, for a streaming method's reply headers.
#[derive(Debug)]
pub struct Response<T> {
    pub(crate) message: T,
    pub(crate) metadata: MetadataMap,
}

impl<T> Response<T> {
    pub fn new(message: T) -> Self {
        Response { message, metadata: MetadataMap::new() }
    }

    /// One ASCII metadata entry on the reply's headers; a key or value that is not valid ASCII
    /// metadata is dropped and logged at `warn`. A key written twice carries both values.
    pub fn metadata(mut self, key: &'static str, value: &str) -> Self {
        match (MetadataKey::<Ascii>::from_bytes(key.as_bytes()), MetadataValue::<Ascii>::try_from(value)) {
            (Ok(name), Ok(value)) => {
                self.metadata.append(name, value);
            }
            _ => tracing::warn!(key, "reply metadata dropped: the key or the value is not valid ASCII metadata"),
        }
        self
    }

    pub fn into_inner(self) -> T {
        self.message
    }
}

/// A reply of one message, what a unary or client-streaming handler returns and
/// [`GrpcCx::reply`] takes: the message, or [`Response`] around it with reply metadata. `()` is
/// one too, being prost's `google.protobuf.Empty`.
pub trait ReplyValue: Send + 'static {
    type Message;
    fn into_grpc(self) -> Reply;
}

impl<T: prost::Message + Default + Send + 'static> ReplyValue for T {
    type Message = T;

    fn into_grpc(self) -> Reply {
        dispatch::encode_one(self, MetadataMap::new())
    }
}

impl<T: prost::Message + Default + Send + 'static> ReplyValue for Response<T> {
    type Message = T;

    fn into_grpc(self) -> Reply {
        dispatch::encode_one(self.message, self.metadata)
    }
}

/// A reply stream, what a server-streaming or bidi handler returns and [`GrpcCx::reply_stream`]
/// takes: any stream of [`ReplyItem`]s, or [`Response`] around one with reply metadata.
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
        dispatch::encode_stream(self, MetadataMap::new(), cx)
    }
}

impl<S> ReplyStream for Response<S>
where
    S: Stream + Send + 'static,
    S::Item: ReplyItem,
{
    type Message = <S::Item as ReplyItem>::Message;

    fn into_grpc(self, cx: &GrpcCx) -> Reply {
        dispatch::encode_stream(self.message, self.metadata, cx)
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
