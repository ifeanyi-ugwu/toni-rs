use std::sync::Arc;

use crate::http_helpers::RouteMetadata;
use crate::traits_helpers::validate::Validatable;

use super::{CancellationToken, Extensions};

/// State shared by every per-transport handler context.
///
/// Every concrete handler context (`HttpContext`, `RpcContext`, `WsContext`)
/// holds one of these and delegates the universal `HandlerContext` methods to it.
/// Keeping the shared bits in one struct avoids per-context boilerplate and
/// keeps the `HandlerContext` impls a thin delegation.
pub(crate) struct SharedState {
    pub(crate) route_metadata: Option<Arc<RouteMetadata>>,
    pub(crate) abort: bool,
    pub(crate) dto: Option<Box<dyn Validatable>>,
    pub(crate) extensions: Extensions,
    pub(crate) cancellation: CancellationToken,
}

impl SharedState {
    pub(crate) fn new(route_metadata: Option<Arc<RouteMetadata>>) -> Self {
        Self::with_extensions(route_metadata, Extensions::new())
    }

    /// Build around a bag that already exists — the HTTP path, where the bag is
    /// created at the adapter seam and rides the request into the context.
    pub(crate) fn with_extensions(
        route_metadata: Option<Arc<RouteMetadata>>,
        extensions: Extensions,
    ) -> Self {
        Self {
            route_metadata,
            abort: false,
            dto: None,
            extensions,
            cancellation: CancellationToken::new(),
        }
    }
}
