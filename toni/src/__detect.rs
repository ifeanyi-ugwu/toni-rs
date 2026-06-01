//! Compile-time detection of which enhancer traits a provider implements.
//!
//! The generated provider factory uses these probes to populate role registrations without a
//! `#[guard]`/`#[interceptor]`/`#[pipe]`/`#[error_handler]` marker: the fact that a type
//! `impl`s `Guard<HttpContext>` is the declaration.
//!
//! Each probe pairs an inherent method — present only when `T: SomeTrait` — with a blanket trait
//! method returning `None`. Inherent methods win method resolution, so where `T` is concrete (the
//! factory's `build()`), `Probe(arc).detect()` yields the coerced `Arc<dyn SomeTrait>` for an
//! implementor and `None` otherwise.
//!
//! This only works where `T` is concrete. Wrapping a probe in a generic `fn detect<T>(..)` erases
//! the bound — inside such a function `T` is unconstrained, so the inherent method never applies
//! and every call returns `None`. The factory therefore emits the probe call inline at the
//! monomorphic site, with [`prelude`] in scope for the fallback methods.

#![doc(hidden)]

use std::sync::Arc;

use crate::context::{GrpcContext, HttpContext, RpcContext, WsContext};
use crate::grpc_status::GrpcStatus;
use crate::http_helpers::HttpResponse;
use crate::rpc::RpcData;
use crate::traits_helpers::middleware::Middleware;
use crate::traits_helpers::{ErrorHandler, Guard, Interceptor, Pipe};
use crate::websocket::WsMessage;

/// Define a probe: an inherent `detect` (gated on `$bound`) that coerces to `Arc<$out>`, shadowing
/// a blanket fallback `detect` that returns `None`.
macro_rules! probe {
    ($probe:ident, $fallback:ident, $bound:path, $out:ty) => {
        pub struct $probe<T>(pub Arc<T>);

        impl<T: $bound + 'static> $probe<T> {
            #[inline]
            pub fn detect(&self) -> Option<Arc<$out>> {
                Some(self.0.clone() as Arc<$out>)
            }
        }

        pub trait $fallback {
            fn detect(&self) -> Option<Arc<$out>>;
        }

        impl<T> $fallback for $probe<T> {
            #[inline]
            fn detect(&self) -> Option<Arc<$out>> {
                None
            }
        }
    };
}

probe!(HttpGuardProbe, HttpGuardProbeFallback, Guard<HttpContext>, dyn Guard<HttpContext>);
probe!(RpcGuardProbe, RpcGuardProbeFallback, Guard<RpcContext>, dyn Guard<RpcContext>);
probe!(WsGuardProbe, WsGuardProbeFallback, Guard<WsContext>, dyn Guard<WsContext>);
probe!(GrpcGuardProbe, GrpcGuardProbeFallback, Guard<GrpcContext>, dyn Guard<GrpcContext>);

probe!(HttpInterceptorProbe, HttpInterceptorProbeFallback, Interceptor<HttpContext>, dyn Interceptor<HttpContext>);
probe!(RpcInterceptorProbe, RpcInterceptorProbeFallback, Interceptor<RpcContext>, dyn Interceptor<RpcContext>);
probe!(WsInterceptorProbe, WsInterceptorProbeFallback, Interceptor<WsContext>, dyn Interceptor<WsContext>);
probe!(GrpcInterceptorProbe, GrpcInterceptorProbeFallback, Interceptor<GrpcContext>, dyn Interceptor<GrpcContext>);

probe!(HttpPipeProbe, HttpPipeProbeFallback, Pipe<HttpContext>, dyn Pipe<HttpContext>);
probe!(RpcPipeProbe, RpcPipeProbeFallback, Pipe<RpcContext>, dyn Pipe<RpcContext>);
probe!(WsPipeProbe, WsPipeProbeFallback, Pipe<WsContext>, dyn Pipe<WsContext>);

probe!(HttpErrorHandlerProbe, HttpErrorHandlerProbeFallback, ErrorHandler<HttpContext, HttpResponse>, dyn ErrorHandler<HttpContext, HttpResponse>);
probe!(RpcErrorHandlerProbe, RpcErrorHandlerProbeFallback, ErrorHandler<RpcContext, RpcData>, dyn ErrorHandler<RpcContext, RpcData>);
probe!(WsErrorHandlerProbe, WsErrorHandlerProbeFallback, ErrorHandler<WsContext, WsMessage>, dyn ErrorHandler<WsContext, WsMessage>);
probe!(GrpcErrorHandlerProbe, GrpcErrorHandlerProbeFallback, ErrorHandler<GrpcContext, GrpcStatus>, dyn ErrorHandler<GrpcContext, GrpcStatus>);

probe!(MiddlewareProbe, MiddlewareProbeFallback, Middleware, dyn Middleware);

/// Brings every fallback trait into scope so the inline `detect()` calls resolve. Glob-import this
/// once where the probe calls are emitted; method resolution is by receiver type, so the many
/// same-named `detect` methods never collide.
pub mod prelude {
    pub use super::{
        GrpcErrorHandlerProbeFallback, GrpcGuardProbeFallback, GrpcInterceptorProbeFallback,
        HttpErrorHandlerProbeFallback, HttpGuardProbeFallback, HttpInterceptorProbeFallback,
        HttpPipeProbeFallback, MiddlewareProbeFallback, RpcErrorHandlerProbeFallback,
        RpcGuardProbeFallback, RpcInterceptorProbeFallback, RpcPipeProbeFallback,
        WsErrorHandlerProbeFallback, WsGuardProbeFallback, WsInterceptorProbeFallback,
        WsPipeProbeFallback,
    };
}
