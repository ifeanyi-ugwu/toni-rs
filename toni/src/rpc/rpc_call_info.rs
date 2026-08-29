use crate::context::Extensions;
use std::collections::HashMap;

/// Wire-level call info passed by RPC adapters into the framework dispatcher.
///
/// Carries the pattern (subject / topic / channel / method name), per-call
/// metadata (NATS headers, TCP envelope fields), and the extension bag the
/// execution's context adopts. Distinct from
/// [`crate::context::RpcContext`], which is the framework-built handler
/// context carrying declared metadata, the cancellation token, and the
/// execution cache alongside these fields.
#[derive(Debug, Clone)]
pub struct RpcCallInfo {
    /// Transport-specific pattern/topic/channel identifier.
    pub pattern: String,

    /// The wire fields the message arrived with — headers, user properties, record headers,
    /// whatever the transport calls them.
    pub headers: HashMap<String, String>,

    /// Transport-specific extensions. The dispatcher seeds the execution's
    /// bag with this handle, so a value the adapter inserts here is readable
    /// through `ctx.extensions()` in guards, interceptors, and the handler.
    pub extensions: Extensions,
}

impl RpcCallInfo {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            headers: HashMap::new(),
            extensions: Extensions::new(),
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
