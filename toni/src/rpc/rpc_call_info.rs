use crate::http_helpers::Extensions;
use std::collections::HashMap;

/// Wire-level call info passed by RPC adapters into the framework dispatcher.
///
/// Carries the pattern (subject / topic / channel / method name), per-call
/// metadata (NATS headers, TCP envelope fields), and any transport-specific
/// extensions the adapter wants to surface. Distinct from
/// [`crate::context::RpcContext`], which is the framework-built handler
/// context and additionally carries route metadata, the typed extensions
/// bag, the cancellation token, the abort flag, the request data, and the
/// eventual response.
#[derive(Debug, Clone)]
pub struct RpcCallInfo {
    /// Transport-specific pattern/topic/channel identifier.
    pub pattern: String,

    /// Message metadata (headers, properties, etc.).
    pub metadata: HashMap<String, String>,

    /// Type-erased transport-specific extensions.
    pub extensions: Extensions,
}

impl RpcCallInfo {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            metadata: HashMap::new(),
            extensions: Extensions::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }
}
