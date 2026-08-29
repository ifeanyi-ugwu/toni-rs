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
/// The same constraint decides how a handler sees this context at all. A gRPC handler's signature
/// is the tonic trait's and never includes one, so guards, interceptors and error handlers receive
/// it as a parameter and a handler takes it off the request instead — [`GrpcContext::of`], or
/// `Extensions::adopt(request.extensions())` for the bag alone.
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

    /// The context riding a gRPC request, which is how a handler reaches one.
    ///
    /// `#[grpc_methods]` puts it there before the handler runs, so this answers
    /// `Some` for any service the framework dispatches. A service handed to
    /// tonic directly through `GrpcAdapter::add_service` has no execution
    /// behind it and answers `None`.
    ///
    /// ```ignore
    /// async fn watch(&self, request: Request<WatchRequest>)
    ///     -> Result<Response<Self::WatchStream>, Status>
    /// {
    ///     let ctx = GrpcContext::of(request.extensions()).expect("dispatched by toni");
    ///     let cancelled = ctx.cancellation().clone();
    ///     // …stop feeding the reply once `cancelled.cancelled()` resolves
    /// }
    /// ```
    pub fn of(carrier: &http::Extensions) -> Option<Self> {
        carrier.get::<Self>().cloned()
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
