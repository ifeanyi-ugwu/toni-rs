use std::sync::Arc;

use crate::context::Metadata;

use super::{CancellationToken, Extensions};
use crate::traits_helpers::ExecutionCache;

/// State shared by every per-transport handler context.
///
/// Every concrete handler context (`HttpContext`, `RpcContext`, `WsContext`)
/// holds one of these and delegates the universal `HandlerContext` methods to it.
/// Keeping the shared bits in one struct avoids per-context boilerplate and
/// keeps the `HandlerContext` impls a thin delegation.
pub(crate) struct SharedState {
    pub(crate) metadata: Option<Arc<Metadata>>,
    pub(crate) extensions: Extensions,
    pub(crate) cancellation: CancellationToken,
    pub(crate) cache: ExecutionCache,
}

impl SharedState {
    pub(crate) fn new(metadata: Option<Arc<Metadata>>) -> Self {
        Self::with_extensions(metadata, Extensions::new())
    }

    /// Build around a bag that already exists — the HTTP path, where the bag is
    /// created at the adapter seam and rides the request into the context.
    pub(crate) fn with_extensions(metadata: Option<Arc<Metadata>>, extensions: Extensions) -> Self {
        Self {
            metadata,
            extensions,
            cancellation: CancellationToken::new(),
            cache: ExecutionCache::new(),
        }
    }
}
