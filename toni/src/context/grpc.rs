use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::context::Metadata;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for gRPC handlers.
///
/// gRPC payloads are method-typed protobuf messages and can't sit in a
/// non-generic struct, so the context deliberately holds only what every
/// enhancer can name without a type parameter: the method path, the
/// inbound metadata (ASCII headers), and the optional peer address.
///
/// Pipes that need to transform the request body are not supported on gRPC
/// for the same reason — the body is method-typed and there's no
/// typed place to put it.
///
/// The same constraint decides who sees this context at all. A gRPC handler's signature is the
/// tonic trait's and never includes it, so guards, interceptors and error handlers are the only
/// participants that receive one — and the only ones that can read what `#[set_metadata]` declared
/// on the service. A handler reaches the extension bag instead, through
/// `Extensions::adopt(request.extensions())`.
#[derive(Clone)]
pub struct GrpcContext {
    inner: Arc<GrpcInner>,
}

struct GrpcInner {
    shared: SharedState,
    method: String,
    headers: HashMap<String, String>,
    peer: Option<SocketAddr>,
}

impl GrpcContext {
    pub fn new(
        method: impl Into<String>,
        headers: HashMap<String, String>,
        peer: Option<SocketAddr>,
        metadata: Option<Arc<Metadata>>,
    ) -> Self {
        Self {
            inner: Arc::new(GrpcInner {
                shared: SharedState::new(metadata),
                method: method.into(),
                headers,
                peer,
            }),
        }
    }

    /// Full method path, e.g. `"orders.OrdersService/CreateOrder"`.
    pub fn method(&self) -> &str {
        &self.inner.method
    }

    /// The wire fields that arrived with this call.
    ///
    /// gRPC's specification calls these metadata; `headers` is the one name this framework uses for all of them, leaving `metadata`
    /// to mean what `#[set_metadata]` declared.
    #[doc(alias = "metadata")]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.inner.headers
    }

    /// One wire field by key.
    #[doc(alias = "get_metadata")]
    #[doc(alias = "metadata")]
    pub fn header(&self, key: &str) -> Option<&str> {
        self.inner.headers.get(key).map(|s| s.as_str())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }
}

impl HandlerContext for GrpcContext {
    fn metadata(&self) -> Option<&Metadata> {
        self.inner.shared.metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cache(&self) -> &crate::traits_helpers::ExecutionCache {
        &self.inner.shared.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }

    // `deadline()` from `grpc-timeout` would be a useful override here but
    // requires parsing the wire-format timeout suffix; deferred until an
    // enhancer wants to read it.
}
