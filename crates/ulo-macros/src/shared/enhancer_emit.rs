//! Single source of truth for the per-transport enhancer emission shape.
//!
//! Three transports × two enhancer roles (Guard / Interceptor) gives six
//! `EnhancerKind` variants, plus gRPC's two. Three transports × ErrorHandler gives three
//! `ErrorHandlerKind` variants. ErrorHandlers don't have entry-wrapping or
//! dyn-factory shapes, so they're a separate small kind to keep `EnhancerKind`
//! uniform.
//!
//! Every emission site (singleton role-push, execution-scoped dyn-factory) reads
//! from these specs instead of restating the per-variant constants inline.

use proc_macro2::TokenStream;
use quote::quote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhancerKind {
    HttpGuard,
    HttpInterceptor,
    RpcGuard,
    RpcInterceptor,
    WsGuard,
    WsInterceptor,
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
    /// `::ulo::spi::ProviderRole::HttpGuard` etc.
    pub role_variant: TokenStream,
    /// `::ulo::__enhancer::GuardEntry::<::ulo::__enhancer::Http>` etc.
    pub entry_path: TokenStream,
    /// `::ulo::enhancer::Guard<::ulo::http::HttpContext>` etc.
    pub trait_path: TokenStream,
    /// `::ulo::__enhancer::GuardFactory<::ulo::__enhancer::Http>` etc.
    pub factory_trait: TokenStream,
    /// Camel-case suffix used to derive a unique factory struct name per kind.
    pub factory_suffix: &'static str,
    /// `::ulo::http::HttpContext` etc. — what this kind's factory is handed.
    pub context_path: TokenStream,
    /// `::ulo::di::Execution::Http` etc. — how that context is wrapped for
    /// the provider being built.
    pub provider_ctx_variant: TokenStream,
}

pub struct ErrorHandlerSpec {
    /// `::ulo::spi::ProviderRole::HttpErrorHandler` etc.
    pub role_variant: TokenStream,
}

impl EnhancerKind {
    pub fn all() -> [EnhancerKind; 8] {
        use EnhancerKind::*;
        [
            HttpGuard,
            HttpInterceptor,
            RpcGuard,
            RpcInterceptor,
            WsGuard,
            WsInterceptor,
            GrpcGuard,
            GrpcInterceptor,
        ]
    }

    pub fn spec(self) -> EnhancerSpec {
        match self {
            EnhancerKind::HttpGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::HttpGuard },
                entry_path: quote! { ::ulo::__enhancer::GuardEntry::<::ulo::__enhancer::Http> },
                trait_path: quote! { ::ulo::enhancer::Guard<::ulo::http::HttpContext> },
                factory_trait: quote! { ::ulo::__enhancer::GuardFactory<::ulo::__enhancer::Http> },
                factory_suffix: "HttpGuard",
                context_path: quote! { ::ulo::http::HttpContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Http },
            },
            EnhancerKind::HttpInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::HttpInterceptor },
                entry_path: quote! { ::ulo::__enhancer::InterceptorEntry::<::ulo::__enhancer::Http> },
                trait_path: quote! { ::ulo::enhancer::Interceptor<::ulo::http::HttpContext, ::ulo::http::HttpHandlerResult> },
                factory_trait: quote! { ::ulo::__enhancer::InterceptorFactory<::ulo::__enhancer::Http> },
                factory_suffix: "HttpInterceptor",
                context_path: quote! { ::ulo::http::HttpContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Http },
            },
            EnhancerKind::RpcGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::RpcGuard },
                entry_path: quote! { ::ulo::__enhancer::GuardEntry::<::ulo::__enhancer::Rpc> },
                trait_path: quote! { ::ulo::enhancer::Guard<::ulo::rpc::RpcContext> },
                factory_trait: quote! { ::ulo::__enhancer::GuardFactory<::ulo::__enhancer::Rpc> },
                factory_suffix: "RpcGuard",
                context_path: quote! { ::ulo::rpc::RpcContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Rpc },
            },
            EnhancerKind::RpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::RpcInterceptor },
                entry_path: quote! { ::ulo::__enhancer::InterceptorEntry::<::ulo::__enhancer::Rpc> },
                trait_path: quote! { ::ulo::enhancer::Interceptor<::ulo::rpc::RpcContext, ::ulo::rpc::RpcHandlerResult> },
                factory_trait: quote! { ::ulo::__enhancer::InterceptorFactory<::ulo::__enhancer::Rpc> },
                factory_suffix: "RpcInterceptor",
                context_path: quote! { ::ulo::rpc::RpcContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Rpc },
            },
            EnhancerKind::WsGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::WsGuard },
                entry_path: quote! { ::ulo::__enhancer::GuardEntry::<::ulo::__enhancer::Ws> },
                trait_path: quote! { ::ulo::enhancer::Guard<::ulo::ws::WsContext> },
                factory_trait: quote! { ::ulo::__enhancer::GuardFactory<::ulo::__enhancer::Ws> },
                factory_suffix: "WsGuard",
                context_path: quote! { ::ulo::ws::WsContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Ws },
            },
            EnhancerKind::WsInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::WsInterceptor },
                entry_path: quote! { ::ulo::__enhancer::InterceptorEntry::<::ulo::__enhancer::Ws> },
                trait_path: quote! { ::ulo::enhancer::Interceptor<::ulo::ws::WsContext, ::ulo::ws::WsHandlerResult> },
                factory_trait: quote! { ::ulo::__enhancer::InterceptorFactory<::ulo::__enhancer::Ws> },
                factory_suffix: "WsInterceptor",
                context_path: quote! { ::ulo::ws::WsContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Ws },
            },
            EnhancerKind::GrpcGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::GrpcGuard },
                entry_path: quote! { ::ulo::__enhancer::GuardEntry::<::ulo::__enhancer::Grpc> },
                trait_path: quote! { ::ulo::enhancer::Guard<::ulo::grpc::GrpcContext> },
                factory_trait: quote! { ::ulo::__enhancer::GuardFactory<::ulo::__enhancer::Grpc> },
                factory_suffix: "GrpcGuard",
                context_path: quote! { ::ulo::grpc::GrpcContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Grpc },
            },
            EnhancerKind::GrpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::GrpcInterceptor },
                entry_path: quote! { ::ulo::__enhancer::InterceptorEntry::<::ulo::__enhancer::Grpc> },
                trait_path: quote! { ::ulo::enhancer::Interceptor<::ulo::grpc::GrpcContext, ::ulo::grpc::GrpcHandlerResult> },
                factory_trait: quote! { ::ulo::__enhancer::InterceptorFactory<::ulo::__enhancer::Grpc> },
                factory_suffix: "GrpcInterceptor",
                context_path: quote! { ::ulo::grpc::GrpcContext },
                provider_ctx_variant: quote! { ::ulo::di::Execution::Grpc },
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
                role_variant: quote! { ::ulo::spi::ProviderRole::HttpErrorHandler },
            },
            ErrorHandlerKind::Rpc => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::RpcErrorHandler },
            },
            ErrorHandlerKind::Ws => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::WsErrorHandler },
            },
            ErrorHandlerKind::Grpc => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::spi::ProviderRole::GrpcErrorHandler },
            },
        }
    }
}

/// Emit the value-probe detection block that pushes a `ProviderRole` for every enhancer trait the
/// already-built `instance` implements. `instance` (an `Arc<ConcreteType>`) and `__roles`
/// (`Vec<ProviderRole>`) must be in scope; the concrete type must be statically known here, so the
/// `ulo::__detect` autoref probes resolve (a generic wrapper would erase the bound and detect
/// nothing — see the `__detect` module docs).
///
/// Used by the `#[injectable]` factory's singleton role registration. Middleware, the eight
/// guard/interceptor kinds, and the four error-handler kinds are each probed; only implemented ones
/// register.
pub fn value_probe_detection() -> TokenStream {
    let mut detects = vec![quote! {
        if let Some(__r) = ::ulo::__detect::MiddlewareProbe(instance.clone()).detect() {
            __roles.push(::ulo::spi::ProviderRole::Middleware(__r));
        }
    }];

    for kind in EnhancerKind::all() {
        let spec = kind.spec();
        let probe = quote::format_ident!("{}Probe", spec.factory_suffix);
        let role_variant = &spec.role_variant;
        let entry_path = &spec.entry_path;
        detects.push(quote! {
            if let Some(__r) = ::ulo::__detect::#probe(instance.clone()).detect() {
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
            if let Some(__r) = ::ulo::__detect::#probe(instance.clone()).detect() {
                __roles.push(#role_variant(__r));
            }
        });
    }

    quote! {
        {
            use ::ulo::__detect::prelude::*;
            #(#detects)*
        }
    }
}
