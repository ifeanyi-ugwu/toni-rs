use crate::context::{GrpcContext, HttpContext, RpcContext, WsContext};

/// The execution a provider is being built for.
///
/// Passed to [`Provider::execute`](crate::traits_helpers::Provider::execute) so a
/// request-scoped provider can reach the execution it belongs to — its cache,
/// its extension bag, and whatever the transport carries.
///
/// Each variant holds the transport's context handle, which is cheap to clone.
/// State every execution has is reached through
/// [`HandlerContext`](crate::context::HandlerContext) whichever variant this is;
/// state one transport has is reached by matching.
#[derive(Clone)]
pub enum ProviderContext {
    Http(HttpContext),
    WebSocket(WsContext),
    Rpc(RpcContext),
    Grpc(GrpcContext),
    /// No active execution — module initialisation, `ApplicationContext::get`.
    None,
}

impl ProviderContext {
    /// The execution's instance cache, or `None` outside an execution.
    ///
    /// This is what a request-scoped provider needs and the only thing it needs
    /// from every transport, which is why it is reachable without matching.
    pub fn cache(&self) -> Option<&crate::traits_helpers::ExecutionCache> {
        use crate::context::HandlerContext;
        match self {
            Self::Http(c) => Some(c.cache()),
            Self::WebSocket(c) => Some(c.cache()),
            Self::Rpc(c) => Some(c.cache()),
            Self::Grpc(c) => Some(c.cache()),
            Self::None => None,
        }
    }

    /// The execution's extension bag, or `None` outside an execution.
    pub fn extensions(&self) -> Option<crate::context::Extensions> {
        use crate::context::HandlerContext;
        match self {
            Self::Http(c) => Some(c.extensions().clone()),
            Self::WebSocket(c) => Some(c.extensions().clone()),
            Self::Rpc(c) => Some(c.extensions().clone()),
            Self::Grpc(c) => Some(c.extensions().clone()),
            Self::None => None,
        }
    }

    /// Refuses a provider whose scope this execution cannot satisfy.
    ///
    /// A request-scoped instance lives in the execution's cache. Where there is no
    /// execution there is nowhere to put it, and the generated provider panics on
    /// the missing cache — so a caller resolving by hand checks here first and
    /// returns the refusal instead.
    pub(crate) fn ensure_can_build(
        &self,
        scope: crate::ProviderScope,
        token: &str,
    ) -> anyhow::Result<()> {
        if scope == crate::ProviderScope::Request && self.cache().is_none() {
            return Err(anyhow::anyhow!(
                "Provider '{}' is request-scoped and cannot be built outside an execution. \
                 Resolve it in one with `resolve` on the application.",
                token
            ));
        }

        Ok(())
    }

    /// The HTTP request parts, when this execution is an HTTP one.
    pub fn request_parts(&self) -> Option<&crate::http_helpers::RequestPart> {
        match self {
            Self::Http(c) => Some(c.request()),
            _ => None,
        }
    }
}
