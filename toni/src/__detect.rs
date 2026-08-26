//! Compile-time detection of which enhancer traits a provider implements.
//!
//! The generated provider factory uses these probes to populate role registrations without a
//! `#[guard]`/`#[interceptor]`/`#[error_handler]` marker: the fact that a type
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

use std::marker::PhantomData;
use std::sync::Arc;

use crate::context::{GrpcContext, HttpContext, RpcContext, WsContext};
use crate::grpc_status::GrpcStatus;
use crate::http_helpers::HttpResponse;
use crate::rpc::RpcData;
use crate::traits_helpers::middleware::Middleware;
use crate::traits_helpers::{ErrorHandler, Guard, Interceptor};
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

probe!(
    HttpGuardProbe,
    HttpGuardProbeFallback,
    Guard<HttpContext>,
    dyn Guard<HttpContext>
);
probe!(
    RpcGuardProbe,
    RpcGuardProbeFallback,
    Guard<RpcContext>,
    dyn Guard<RpcContext>
);
probe!(
    WsGuardProbe,
    WsGuardProbeFallback,
    Guard<WsContext>,
    dyn Guard<WsContext>
);
probe!(
    GrpcGuardProbe,
    GrpcGuardProbeFallback,
    Guard<GrpcContext>,
    dyn Guard<GrpcContext>
);

probe!(
    HttpInterceptorProbe,
    HttpInterceptorProbeFallback,
    Interceptor<HttpContext, HttpResponse>,
    dyn Interceptor<HttpContext, HttpResponse>
);
probe!(
    RpcInterceptorProbe,
    RpcInterceptorProbeFallback,
    Interceptor<RpcContext, crate::rpc::RpcHandlerResult>,
    dyn Interceptor<RpcContext, crate::rpc::RpcHandlerResult>
);
probe!(
    WsInterceptorProbe,
    WsInterceptorProbeFallback,
    Interceptor<WsContext, crate::websocket::WsHandlerResult>,
    dyn Interceptor<WsContext, crate::websocket::WsHandlerResult>
);
probe!(
    GrpcInterceptorProbe,
    GrpcInterceptorProbeFallback,
    Interceptor<GrpcContext, crate::GrpcHandlerResult>,
    dyn Interceptor<GrpcContext, crate::GrpcHandlerResult>
);

probe!(HttpErrorHandlerProbe, HttpErrorHandlerProbeFallback, ErrorHandler<HttpContext, HttpResponse>, dyn ErrorHandler<HttpContext, HttpResponse>);
probe!(RpcErrorHandlerProbe, RpcErrorHandlerProbeFallback, ErrorHandler<RpcContext, RpcData>, dyn ErrorHandler<RpcContext, RpcData>);
probe!(WsErrorHandlerProbe, WsErrorHandlerProbeFallback, ErrorHandler<WsContext, WsMessage>, dyn ErrorHandler<WsContext, WsMessage>);
probe!(GrpcErrorHandlerProbe, GrpcErrorHandlerProbeFallback, ErrorHandler<GrpcContext, GrpcStatus>, dyn ErrorHandler<GrpcContext, GrpcStatus>);

probe!(
    MiddlewareProbe,
    MiddlewareProbeFallback,
    Middleware,
    dyn Middleware
);

/// Define a type-level probe: `is()` returns `true` when `T: $bound`, `false` otherwise, with no
/// instance. The request/transient path needs this to decide — in the factory's `build()`, where
/// `T` is concrete — whether to register a per-request enhancer factory before any instance exists.
macro_rules! type_probe {
    ($probe:ident, $fallback:ident, $bound:path) => {
        pub struct $probe<T>(pub PhantomData<T>);

        impl<T: $bound + 'static> $probe<T> {
            #[inline]
            pub fn is(&self) -> bool {
                true
            }
        }

        pub trait $fallback {
            fn is(&self) -> bool;
        }

        impl<T> $fallback for $probe<T> {
            #[inline]
            fn is(&self) -> bool {
                false
            }
        }
    };
}

type_probe!(
    HttpGuardTypeProbe,
    HttpGuardTypeProbeFallback,
    Guard<HttpContext>
);
type_probe!(
    RpcGuardTypeProbe,
    RpcGuardTypeProbeFallback,
    Guard<RpcContext>
);
type_probe!(WsGuardTypeProbe, WsGuardTypeProbeFallback, Guard<WsContext>);
type_probe!(
    GrpcGuardTypeProbe,
    GrpcGuardTypeProbeFallback,
    Guard<GrpcContext>
);

type_probe!(
    HttpInterceptorTypeProbe,
    HttpInterceptorTypeProbeFallback,
    Interceptor<HttpContext, HttpResponse>
);
type_probe!(
    RpcInterceptorTypeProbe,
    RpcInterceptorTypeProbeFallback,
    Interceptor<RpcContext, crate::rpc::RpcHandlerResult>
);
type_probe!(
    WsInterceptorTypeProbe,
    WsInterceptorTypeProbeFallback,
    Interceptor<WsContext, crate::websocket::WsHandlerResult>
);
type_probe!(
    GrpcInterceptorTypeProbe,
    GrpcInterceptorTypeProbeFallback,
    Interceptor<GrpcContext, crate::GrpcHandlerResult>
);

/// Brings every fallback trait into scope so the inline `detect()` / `is()` calls resolve.
/// Glob-import this once where the probe calls are emitted; method resolution is by receiver type,
/// so the many same-named methods never collide.
pub mod prelude {
    pub use super::{
        GrpcErrorHandlerProbeFallback, GrpcGuardProbeFallback, GrpcGuardTypeProbeFallback,
        GrpcInterceptorProbeFallback, GrpcInterceptorTypeProbeFallback,
        HttpErrorHandlerProbeFallback, HttpGuardProbeFallback, HttpGuardTypeProbeFallback,
        HttpInterceptorProbeFallback, HttpInterceptorTypeProbeFallback, MiddlewareProbeFallback,
        RpcErrorHandlerProbeFallback, RpcGuardProbeFallback, RpcGuardTypeProbeFallback,
        RpcInterceptorProbeFallback, RpcInterceptorTypeProbeFallback, WsErrorHandlerProbeFallback,
        WsGuardProbeFallback, WsGuardTypeProbeFallback, WsInterceptorProbeFallback,
        WsInterceptorTypeProbeFallback,
    };
}
