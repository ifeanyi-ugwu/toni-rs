use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::http_helpers::RouteMetadata;

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
/// `Box<dyn Validatable>`-shaped place to put it.
#[derive(Clone)]
pub struct GrpcContext {
    inner: Arc<GrpcInner>,
}

struct GrpcInner {
    shared: SharedState,
    method: String,
    metadata: HashMap<String, String>,
    peer: Option<SocketAddr>,
}

impl GrpcContext {
    pub fn new(
        method: impl Into<String>,
        metadata: HashMap<String, String>,
        peer: Option<SocketAddr>,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        Self {
            inner: Arc::new(GrpcInner {
                shared: SharedState::new(route_metadata),
                method: method.into(),
                metadata,
                peer,
            }),
        }
    }

    /// Full method path, e.g. `"orders.OrdersService/CreateOrder"`.
    pub fn method(&self) -> &str {
        &self.inner.method
    }

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.inner.metadata
    }

    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        self.inner.metadata.get(key).map(|s| s.as_str())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }
}

impl HandlerContext for GrpcContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.inner.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }

    // `deadline()` from `grpc-timeout` would be a useful override here but
    // requires parsing the wire-format timeout suffix; deferred until an
    // enhancer wants to read it.
}
