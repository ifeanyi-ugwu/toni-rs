use std::collections::HashMap;
use std::sync::Arc;

use crate::http_helpers::{Extensions as TransportExtensions, RouteMetadata};
use crate::rpc::{RpcData, RpcError};

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for RPC handlers.
///
/// Owns the inbound payload, the call's pattern (subject/topic/method name),
/// transport metadata (NATS headers, TCP envelope fields, etc.), and the
/// (eventual) response.
///
/// Folds in what was previously the standalone `rpc::RpcContext` so there is
/// one type in flight per request, not two.
pub struct RpcContext {
    pub(crate) shared: SharedState,
    pub(crate) pattern: String,
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) transport_extensions: TransportExtensions,
    pub(crate) data: RpcData,
    pub(crate) response: Option<Result<Option<RpcData>, RpcError>>,
}

impl RpcContext {
    pub fn new(
        pattern: impl Into<String>,
        data: RpcData,
        route_metadata: Option<Arc<RouteMetadata>>,
    ) -> Self {
        Self {
            shared: SharedState::new(route_metadata),
            pattern: pattern.into(),
            metadata: HashMap::new(),
            transport_extensions: TransportExtensions::new(),
            data,
            response: None,
        }
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn metadata_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.metadata
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }

    /// Transport-specific extensions (e.g. the original NATS message handle).
    /// Distinct from [`extensions`](HandlerContext::extensions), which is for
    /// cross-cutting per-request data attached by enhancers.
    pub fn transport_extensions(&self) -> &TransportExtensions {
        &self.transport_extensions
    }

    pub fn transport_extensions_mut(&mut self) -> &mut TransportExtensions {
        &mut self.transport_extensions
    }

    pub fn data(&self) -> &RpcData {
        &self.data
    }

    pub fn response(&self) -> Option<&Result<Option<RpcData>, RpcError>> {
        self.response.as_ref()
    }

    pub fn set_response(&mut self, response: Result<Option<RpcData>, RpcError>) {
        self.response = Some(response);
    }

    pub fn take_response(&mut self) -> Option<Result<Option<RpcData>, RpcError>> {
        self.response.take()
    }
}

impl HandlerContext for RpcContext {
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
}
