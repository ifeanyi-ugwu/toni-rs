use std::collections::HashMap;
use std::sync::Arc;

use crate::context::Metadata;
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
    headers: HashMap<String, String>,
    data: RpcData,
}

impl RpcContext {
    pub fn new(
        pattern: impl Into<String>,
        data: RpcData,
        headers: HashMap<String, String>,
        metadata: Option<Arc<Metadata>>,
    ) -> Self {
        Self::with_extensions(pattern, data, headers, metadata, Extensions::new())
    }

    /// Build around a bag that already exists — the adapter seam's bag riding
    /// the call into the context, so a value the transport inserted is
    /// readable through `extensions()`.
    pub fn with_extensions(
        pattern: impl Into<String>,
        data: RpcData,
        headers: HashMap<String, String>,
        metadata: Option<Arc<Metadata>>,
        extensions: Extensions,
    ) -> Self {
        Self {
            inner: Arc::new(RpcInner {
                shared: SharedState::with_extensions(metadata, extensions),
                pattern: pattern.into(),
                headers,
                data,
            }),
        }
    }

    pub fn pattern(&self) -> &str {
        &self.inner.pattern
    }

    /// The wire fields that arrived with this call.
    ///
    /// NATS headers, AMQP headers, Kafka record headers and MQTT user properties; `headers` is the one name this framework uses for all of them, leaving `metadata`
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

    pub fn data(&self) -> &RpcData {
        &self.inner.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct Stamp(u8);

    #[test]
    fn the_adapters_bag_is_the_contexts_bag() {
        let bag = Extensions::new();
        bag.insert(Stamp(7));
        let ctx =
            RpcContext::with_extensions("p", RpcData::text(""), HashMap::new(), None, bag.clone());
        assert_eq!(ctx.extensions().get::<Stamp>(), Some(Stamp(7)));
        ctx.extensions().insert(Stamp(9));
        assert_eq!(bag.get::<Stamp>(), Some(Stamp(9)));
    }
}

impl HandlerContext for RpcContext {
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
}
