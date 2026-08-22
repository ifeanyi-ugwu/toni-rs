use crate::type_map::TypeMap;
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

    /// The wire fields the message arrived with — headers, user properties, record headers,
    /// whatever the transport calls them.
    pub headers: HashMap<String, String>,

    /// Type-erased transport-specific extensions.
    pub extensions: TypeMap,
}

impl RpcCallInfo {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            headers: HashMap::new(),
            extensions: TypeMap::new(),
        }
    }

    #[doc(alias = "with_metadata")]
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    #[doc(alias = "get_metadata")]
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| s.as_str())
    }
}
