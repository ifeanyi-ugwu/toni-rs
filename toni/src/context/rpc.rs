use std::collections::HashMap;
use std::sync::Arc;

use crate::http_helpers::RouteMetadata;
use crate::rpc::RpcData;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for RPC handlers.
///
/// Owns the inbound payload, the call's pattern (subject/topic/method name),
/// and transport metadata (NATS headers, TCP envelope fields, etc.). A handler
/// answers by returning, not by writing here.
#[derive(Clone)]
pub struct RpcContext {
    inner: Arc<RpcInner>,
}

struct RpcInner {
    shared: SharedState,
    pattern: String,
    metadata: HashMap<String, String>,
    data: RpcData,
}

impl RpcContext {
    pub fn new(
        pattern: impl Into<String>,
        data: RpcData,
        metadata: HashMap<String, String>,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        Self {
            inner: Arc::new(RpcInner {
                shared: SharedState::new(route_metadata),
                pattern: pattern.into(),
                metadata,
                data,
            }),
        }
    }

    pub fn pattern(&self) -> &str {
        &self.inner.pattern
    }

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.inner.metadata
    }

    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        self.inner.metadata.get(key).map(|s| s.as_str())
    }

    pub fn data(&self) -> &RpcData {
        &self.inner.data
    }
}

impl HandlerContext for RpcContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.inner.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }
}
