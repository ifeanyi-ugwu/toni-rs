mod chain;
pub use chain::{ChainLink, FinalHandler, MiddlewareChain};

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
