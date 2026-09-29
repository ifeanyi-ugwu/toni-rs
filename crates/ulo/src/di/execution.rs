use crate::context::StandaloneContext;
use crate::grpc::GrpcContext;
use crate::http::HttpContext;
use crate::rpc::RpcContext;
use crate::ws::WsContext;

/// The execution a provider is being built for.
///
/// Passed to [`Provider::resolve`](crate::spi::Provider::resolve) so a
/// execution-scoped provider can reach the execution it belongs to — its cache,
/// its extension bag, and whatever the transport carries.
///
/// Each variant holds a context handle, which is cheap to clone. State every
/// execution has is reached through
/// [`ExecutionContext`](crate::context::ExecutionContext) whichever variant this is;
/// state one transport has is reached by matching, and a standalone execution has
/// none to match on.
#[derive(Clone)]
#[non_exhaustive]
pub enum Execution {
    Http(HttpContext),
    Ws(WsContext),
    Rpc(RpcContext),
    Grpc(GrpcContext),
    /// An execution with no transport behind it — a CLI command, a job, a test.
    Standalone(StandaloneContext),
    /// No active execution — module initialisation, `ApplicationContext::get`.
    None,
}

impl Execution {
    /// A fresh execution belonging to no transport.
    ///
    /// What a caller resolving providers by hand starts with. Everything resolved
    /// against the returned value shares one cache, so an execution-scoped provider is
    /// built once for all of them; the execution ends when the value is dropped.
    pub fn standalone() -> Self {
        Self::Standalone(StandaloneContext::new())
    }

    /// The execution's instance cache, or `None` outside an execution.
    ///
    /// This is what an execution-scoped provider needs and the only thing it needs
    /// from every transport, which is why it is reachable without matching.
    pub fn cache(&self) -> Option<&crate::di::ExecutionCache> {
        use crate::context::ExecutionContext;
        match self {
            Self::Http(c) => Some(c.cache()),
            Self::Ws(c) => Some(c.cache()),
            Self::Rpc(c) => Some(c.cache()),
            Self::Grpc(c) => Some(c.cache()),
            Self::Standalone(c) => Some(c.cache()),
            Self::None => None,
        }
    }

    /// The execution's extension bag, or `None` outside an execution.
    pub fn extensions(&self) -> Option<crate::context::Extensions> {
        use crate::context::ExecutionContext;
        match self {
            Self::Http(c) => Some(c.extensions().clone()),
            Self::Ws(c) => Some(c.extensions().clone()),
            Self::Rpc(c) => Some(c.extensions().clone()),
            Self::Grpc(c) => Some(c.extensions().clone()),
            Self::Standalone(c) => Some(c.extensions().clone()),
            Self::None => None,
        }
    }

    /// The HTTP request parts, when this execution is an HTTP one.
    pub fn request_parts(&self) -> Option<&crate::http::RequestPart> {
        match self {
            Self::Http(c) => Some(c.request()),
            _ => None,
        }
    }
}

impl From<HttpContext> for Execution {
    fn from(ctx: HttpContext) -> Self {
        Self::Http(ctx)
    }
}

impl From<WsContext> for Execution {
    fn from(ctx: WsContext) -> Self {
        Self::Ws(ctx)
    }
}

impl From<RpcContext> for Execution {
    fn from(ctx: RpcContext) -> Self {
        Self::Rpc(ctx)
    }
}

impl From<GrpcContext> for Execution {
    fn from(ctx: GrpcContext) -> Self {
        Self::Grpc(ctx)
    }
}

impl From<StandaloneContext> for Execution {
    fn from(ctx: StandaloneContext) -> Self {
        Self::Standalone(ctx)
    }
}
