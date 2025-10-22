mod chain;
mod route_pattern;
pub use chain::{ChainLink, FinalHandler, MiddlewareChain};
pub use route_pattern::{IntoRoutePattern, RoutePattern};

pub use crate::http_helpers::HttpMethod;

pub mod builtin;
pub use builtin::{
    AuthMiddleware, CompressionMiddleware, CorsMiddleware, LoggerMiddleware, RateLimitMiddleware,
    TimeoutMiddleware,
};

mod module_middleware;
pub use module_middleware::{MiddlewareConfigurer, MiddlewareManager};

// Re-export core traits
pub use crate::traits_helpers::middleware::{
    FunctionalMiddleware, Middleware, MiddlewareConfiguration, MiddlewareFn, MiddlewareResult, Next,
};
