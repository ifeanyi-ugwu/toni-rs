use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::grpc_status::GrpcStatus;
use crate::http_helpers::RouteMetadata;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for gRPC handlers.
///
/// gRPC payloads are method-typed protobuf messages and can't sit in a
/// non-generic struct, so the context deliberately holds only what every
/// enhancer can name without a type parameter: the method path, the
/// inbound metadata (ASCII headers), the optional peer address, and an
/// error-only response slot for guard / interceptor short-circuit.
///
/// Pipes that need to transform the request body are not supported on gRPC
/// for the same reason — the body is method-typed and there's no
/// `Box<dyn Validatable>`-shaped place to put it.
pub struct GrpcContext {
    pub(crate) shared: SharedState,
    pub(crate) method: String,
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) peer: Option<SocketAddr>,
    /// Set by guards or interceptors to short-circuit the call. The handler
    /// runs only if this stays `None`. Error-only because the success type
    /// is method-specific and can't fit a generic slot.
    pub(crate) response: Option<Result<(), GrpcStatus>>,
}

impl GrpcContext {
    pub fn new(
        method: impl Into<String>,
        metadata: HashMap<String, String>,
        peer: Option<SocketAddr>,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        Self {
            shared: SharedState::new(route_metadata),
            method: method.into(),
            metadata,
            peer,
            response: None,
        }
    }

    /// Full method path, e.g. `"orders.OrdersService/CreateOrder"`.
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn metadata_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.metadata
    }

    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub fn response(&self) -> Option<&Result<(), GrpcStatus>> {
        self.response.as_ref()
    }

    pub fn set_response(&mut self, response: Result<(), GrpcStatus>) {
        self.response = Some(response);
    }

    pub fn take_response(&mut self) -> Option<Result<(), GrpcStatus>> {
        self.response.take()
    }
}

impl HandlerContext for GrpcContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.shared.extensions
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.shared.cancellation
    }

    fn abort(&mut self) {
        self.shared.abort = true;
    }

    fn should_abort(&self) -> bool {
        self.shared.abort
    }

    // `deadline()` from `grpc-timeout` would be a useful override here but
    // requires parsing the wire-format timeout suffix; deferred until an
    // enhancer wants to read it.
}
