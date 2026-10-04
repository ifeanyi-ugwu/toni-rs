use std::net::SocketAddr;
use std::sync::Arc;

use tonic::metadata::MetadataMap;
use ulo::{AppHandle, ExecutionRef, Ext, Extensions, Inputs, LookupError, Timer, Transport};
use ulo_transport::{ExtractError, FromCall};

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
/// service. A stream item's `Err` runs `dispatch_late` and is written in the trailers.
pub type Reply = http::Response<tonic::body::Body>;

/// One call's context: `Clone + Send + Sync`, every clone the same call. The execution, the method
/// path, the request metadata, the peer, and the app's `Timer`.
#[derive(Clone)]
pub struct GrpcCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
}

impl GrpcCx {
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
        todo!()
    }

    /// The request metadata.
    pub fn metadata(&self) -> &GrpcMetadata {
        todo!()
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        todo!()
    }

    pub fn app(&self) -> &AppHandle {
        todo!()
    }

    pub fn timer(&self) -> &Arc<dyn Timer> {
        todo!()
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
}

/// The caller's address, as an execution input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// A reply message with reply metadata: `Response::new(user).metadata("x-cache", "miss")`.
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
    /// metadata is dropped and logged at `warn`.
    pub fn metadata(self, key: &'static str, value: &str) -> Self {
        let _ = (key, value);
        todo!()
    }

    pub fn into_inner(self) -> T {
        self.message
    }
}
