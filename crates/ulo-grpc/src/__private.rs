//! Support for the code `ulo-grpc-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::sync::Arc;

use ulo::{BoxError, BoxFuture, Shape};

pub use ulo_transport as transport;

use crate::method::Method;
use crate::transport::{GrpcCx, Reply};

/// A handler's call: extraction, the controller, the handler and the reply probe, run by
/// `dispatch` after every guard admits.
pub type HandlerFn = Arc<dyn Fn(GrpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// The handler value `#[method(Marker)]` passes to `HandlerSpec::new`, read back from
/// `MountedHandler::handler` by the server: the marker's path and shape, and the call.
pub struct GrpcHandler {
    pub(crate) path: &'static str,
    pub(crate) shape: Shape,
    pub(crate) call: HandlerFn,
}

impl GrpcHandler {
    /// The handler for marker `M`.
    pub fn new<M, F>(call: F) -> Self
    where
        M: Method,
        F: Fn(GrpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync + 'static,
    {
        GrpcHandler { path: M::PATH, shape: M::SHAPE, call: Arc::new(call) }
    }
}

/// The compile-time check of a handler against its marker: the attribute names the handler's
/// message type and reply type at the concrete site, and the bounds fail where they differ.
pub fn check_types<M, Req, Res>()
where
    M: Method<Request = Req, Response = Res>,
{
}
