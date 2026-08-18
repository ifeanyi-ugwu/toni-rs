//! Single source of truth for the per-transport enhancer emission shape.
//!
//! Three transports × three enhancer roles (Guard / Interceptor / Pipe) gives
//! nine `EnhancerKind` variants. Three transports × ErrorHandler gives three
//! `ErrorHandlerKind` variants. ErrorHandlers don't have entry-wrapping or
//! dyn-factory shapes, so they're a separate small kind to keep `EnhancerKind`
//! uniform.
//!
//! Every emission site (singleton role-push, request-scoped dyn-factory,
//! `provider_factory!` ready, `provider_factory!` non-caching factory) reads
//! from these specs instead of restating the per-variant constants inline.

use proc_macro2::TokenStream;
use quote::quote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhancerKind {
    HttpGuard,
    HttpInterceptor,
    HttpPipe,
    RpcGuard,
    RpcInterceptor,
    RpcPipe,
    WsGuard,
    WsInterceptor,
    WsPipe,
    GrpcGuard,
    GrpcInterceptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorHandlerKind {
    Http,
    Rpc,
    Ws,
    Grpc,
}

pub struct EnhancerSpec {
    /// `::toni::traits_helpers::ProviderRole::HttpGuard` etc.
    pub role_variant: TokenStream,
    /// `::toni::traits_helpers::HttpGuardEntry` etc.
    pub entry_path: TokenStream,
    /// `::toni::traits_helpers::Guard<::toni::context::HttpContext>` etc.
    pub trait_path: TokenStream,
    /// `::toni::traits_helpers::DynHttpGuardFactory` etc.
    pub dyn_factory_trait: TokenStream,
    /// Camel-case suffix used to derive a unique factory struct name per kind.
    pub factory_suffix: &'static str,
    /// `::toni::context::HttpContext` etc. — what this kind's factory is handed.
    pub context_path: TokenStream,
    /// `::toni::ProviderContext::Http` etc. — how that context is wrapped for
    /// the provider being built.
    pub provider_ctx_variant: TokenStream,
}

pub struct ErrorHandlerSpec {
    /// `::toni::traits_helpers::ProviderRole::HttpErrorHandler` etc.
    pub role_variant: TokenStream,
}

impl EnhancerKind {
    pub fn all() -> [EnhancerKind; 11] {
        use EnhancerKind::*;
        [
            HttpGuard,
            HttpInterceptor,
            HttpPipe,
            RpcGuard,
            RpcInterceptor,
            RpcPipe,
            WsGuard,
            WsInterceptor,
            WsPipe,
            GrpcGuard,
            GrpcInterceptor,
        ]
    }

    pub fn spec(self) -> EnhancerSpec {
        match self {
            EnhancerKind::HttpGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpGuard },
                entry_path: quote! { ::toni::traits_helpers::HttpGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::HttpContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynHttpGuardFactory },
                factory_suffix: "HttpGuard",
                context_path: quote! { ::toni::context::HttpContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Http },
            },
            EnhancerKind::HttpInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpInterceptor },
                entry_path: quote! { ::toni::traits_helpers::HttpInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::HttpContext, ::toni::http_helpers::HttpResponse> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynHttpInterceptorFactory },
                factory_suffix: "HttpInterceptor",
                context_path: quote! { ::toni::context::HttpContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Http },
            },
            EnhancerKind::HttpPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpPipe },
                entry_path: quote! { ::toni::traits_helpers::HttpPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::HttpContext, ::toni::http_helpers::HttpResponse> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynHttpPipeFactory },
                factory_suffix: "HttpPipe",
                context_path: quote! { ::toni::context::HttpContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Http },
            },
            EnhancerKind::RpcGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcGuard },
                entry_path: quote! { ::toni::traits_helpers::RpcGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::RpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcGuardFactory },
                factory_suffix: "RpcGuard",
                context_path: quote! { ::toni::context::RpcContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Rpc },
            },
            EnhancerKind::RpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcInterceptor },
                entry_path: quote! { ::toni::traits_helpers::RpcInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::RpcContext, ::toni::rpc::RpcHandlerResult> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcInterceptorFactory },
                factory_suffix: "RpcInterceptor",
                context_path: quote! { ::toni::context::RpcContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Rpc },
            },
            EnhancerKind::RpcPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcPipe },
                entry_path: quote! { ::toni::traits_helpers::RpcPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::RpcContext, ::toni::rpc::RpcHandlerResult> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcPipeFactory },
                factory_suffix: "RpcPipe",
                context_path: quote! { ::toni::context::RpcContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Rpc },
            },
            EnhancerKind::WsGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsGuard },
                entry_path: quote! { ::toni::traits_helpers::WsGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::WsContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsGuardFactory },
                factory_suffix: "WsGuard",
                context_path: quote! { ::toni::context::WsContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::WebSocket },
            },
            EnhancerKind::WsInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsInterceptor },
                entry_path: quote! { ::toni::traits_helpers::WsInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::WsContext, ::toni::websocket::WsHandlerResult> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsInterceptorFactory },
                factory_suffix: "WsInterceptor",
                context_path: quote! { ::toni::context::WsContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::WebSocket },
            },
            EnhancerKind::WsPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsPipe },
                entry_path: quote! { ::toni::traits_helpers::WsPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::WsContext, ::toni::websocket::WsHandlerResult> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsPipeFactory },
                factory_suffix: "WsPipe",
                context_path: quote! { ::toni::context::WsContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::WebSocket },
            },
            EnhancerKind::GrpcGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcGuard },
                entry_path: quote! { ::toni::traits_helpers::GrpcGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::GrpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynGrpcGuardFactory },
                factory_suffix: "GrpcGuard",
                context_path: quote! { ::toni::context::GrpcContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Grpc },
            },
            EnhancerKind::GrpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcInterceptor },
                entry_path: quote! { ::toni::traits_helpers::GrpcInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::GrpcContext, ::toni::GrpcHandlerResult> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynGrpcInterceptorFactory },
                factory_suffix: "GrpcInterceptor",
                context_path: quote! { ::toni::context::GrpcContext },
                provider_ctx_variant: quote! { ::toni::ProviderContext::Grpc },
            },
        }
    }
}

impl ErrorHandlerKind {
    pub fn all() -> [ErrorHandlerKind; 4] {
        [
            ErrorHandlerKind::Http,
            ErrorHandlerKind::Rpc,
            ErrorHandlerKind::Ws,
            ErrorHandlerKind::Grpc,
        ]
    }

    pub fn spec(self) -> ErrorHandlerSpec {
        match self {
            ErrorHandlerKind::Http => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpErrorHandler },
            },
            ErrorHandlerKind::Rpc => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcErrorHandler },
            },
            ErrorHandlerKind::Ws => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsErrorHandler },
            },
            ErrorHandlerKind::Grpc => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcErrorHandler },
            },
        }
    }
}

/// Emit the value-probe detection block that pushes a `ProviderRole` for every enhancer trait the
/// already-built `instance` implements. `instance` (an `Arc<ConcreteType>`) and `__roles`
/// (`Vec<ProviderRole>`) must be in scope; the concrete type must be statically known here, so the
/// `toni::__detect` autoref probes resolve (a generic wrapper would erase the bound and detect
/// nothing — see the `__detect` module docs).
///
/// Shared by every singleton role-registration site: the `#[injectable]` factory and the caching
/// `provider_factory!` factory. Middleware, the nine guard/interceptor/pipe
/// kinds, and the four error-handler kinds are each probed; only implemented ones register.
pub fn value_probe_detection() -> TokenStream {
    let mut detects = vec![quote! {
        if let Some(__r) = ::toni::__detect::MiddlewareProbe(instance.clone()).detect() {
            __roles.push(::toni::traits_helpers::ProviderRole::Middleware(__r));
        }
    }];

    for kind in EnhancerKind::all() {
        let spec = kind.spec();
        let probe = quote::format_ident!("{}Probe", spec.factory_suffix);
        let role_variant = &spec.role_variant;
        let entry_path = &spec.entry_path;
        detects.push(quote! {
            if let Some(__r) = ::toni::__detect::#probe(instance.clone()).detect() {
                __roles.push(#role_variant(#entry_path::Ready(__r)));
            }
        });
    }
    for kind in ErrorHandlerKind::all() {
        let spec = kind.spec();
        let probe = match kind {
            ErrorHandlerKind::Http => quote::format_ident!("HttpErrorHandlerProbe"),
            ErrorHandlerKind::Rpc => quote::format_ident!("RpcErrorHandlerProbe"),
            ErrorHandlerKind::Ws => quote::format_ident!("WsErrorHandlerProbe"),
            ErrorHandlerKind::Grpc => quote::format_ident!("GrpcErrorHandlerProbe"),
        };
        let role_variant = &spec.role_variant;
        detects.push(quote! {
            if let Some(__r) = ::toni::__detect::#probe(instance.clone()).detect() {
                __roles.push(#role_variant(__r));
            }
        });
    }

    quote! {
        {
            use ::toni::__detect::prelude::*;
            #(#detects)*
        }
    }
}
