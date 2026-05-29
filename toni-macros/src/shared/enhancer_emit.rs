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
}

pub struct ErrorHandlerSpec {
    /// `::toni::traits_helpers::ProviderRole::HttpErrorHandler` etc.
    pub role_variant: TokenStream,
    /// `::toni::traits_helpers::ErrorHandler<::toni::context::HttpContext, ::toni::http_helpers::HttpResponse>` etc.
    pub trait_path: TokenStream,
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
            },
            EnhancerKind::HttpInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpInterceptor },
                entry_path: quote! { ::toni::traits_helpers::HttpInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::HttpContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynHttpInterceptorFactory },
                factory_suffix: "HttpInterceptor",
            },
            EnhancerKind::HttpPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::HttpPipe },
                entry_path: quote! { ::toni::traits_helpers::HttpPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::HttpContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynHttpPipeFactory },
                factory_suffix: "HttpPipe",
            },
            EnhancerKind::RpcGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcGuard },
                entry_path: quote! { ::toni::traits_helpers::RpcGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::RpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcGuardFactory },
                factory_suffix: "RpcGuard",
            },
            EnhancerKind::RpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcInterceptor },
                entry_path: quote! { ::toni::traits_helpers::RpcInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::RpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcInterceptorFactory },
                factory_suffix: "RpcInterceptor",
            },
            EnhancerKind::RpcPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcPipe },
                entry_path: quote! { ::toni::traits_helpers::RpcPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::RpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynRpcPipeFactory },
                factory_suffix: "RpcPipe",
            },
            EnhancerKind::WsGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsGuard },
                entry_path: quote! { ::toni::traits_helpers::WsGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::WsContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsGuardFactory },
                factory_suffix: "WsGuard",
            },
            EnhancerKind::WsInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsInterceptor },
                entry_path: quote! { ::toni::traits_helpers::WsInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::WsContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsInterceptorFactory },
                factory_suffix: "WsInterceptor",
            },
            EnhancerKind::WsPipe => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsPipe },
                entry_path: quote! { ::toni::traits_helpers::WsPipeEntry },
                trait_path: quote! { ::toni::traits_helpers::Pipe<::toni::context::WsContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynWsPipeFactory },
                factory_suffix: "WsPipe",
            },
            EnhancerKind::GrpcGuard => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcGuard },
                entry_path: quote! { ::toni::traits_helpers::GrpcGuardEntry },
                trait_path: quote! { ::toni::traits_helpers::Guard<::toni::context::GrpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynGrpcGuardFactory },
                factory_suffix: "GrpcGuard",
            },
            EnhancerKind::GrpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcInterceptor },
                entry_path: quote! { ::toni::traits_helpers::GrpcInterceptorEntry },
                trait_path: quote! { ::toni::traits_helpers::Interceptor<::toni::context::GrpcContext> },
                dyn_factory_trait: quote! { ::toni::traits_helpers::DynGrpcInterceptorFactory },
                factory_suffix: "GrpcInterceptor",
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
                trait_path: quote! { ::toni::traits_helpers::ErrorHandler<::toni::context::HttpContext, ::toni::http_helpers::HttpResponse> },
            },
            ErrorHandlerKind::Rpc => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::RpcErrorHandler },
                trait_path: quote! { ::toni::traits_helpers::ErrorHandler<::toni::context::RpcContext, ::toni::rpc::RpcData> },
            },
            ErrorHandlerKind::Ws => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::WsErrorHandler },
                trait_path: quote! { ::toni::traits_helpers::ErrorHandler<::toni::context::WsContext, ::toni::websocket::WsMessage> },
            },
            ErrorHandlerKind::Grpc => ErrorHandlerSpec {
                role_variant: quote! { ::toni::traits_helpers::ProviderRole::GrpcErrorHandler },
                trait_path: quote! { ::toni::traits_helpers::ErrorHandler<::toni::context::GrpcContext, ::toni::grpc_status::GrpcStatus> },
            },
        }
    }
}

/// Emit a `__roles.push(Role::X(Entry::Ready(instance.clone() as Arc<dyn …>)))`
/// statement. `instance` must be in scope at the call site.
pub fn ready_role_push(spec: &EnhancerSpec) -> TokenStream {
    let role = &spec.role_variant;
    let entry = &spec.entry_path;
    let trait_path = &spec.trait_path;
    quote! {
        __roles.push(#role(
            #entry::Ready(
                instance.clone() as ::std::sync::Arc<dyn #trait_path>
            )
        ));
    }
}

/// Emit a `__roles.push(Role::HttpErrorHandler(instance.clone() as Arc<dyn …>))`
/// statement. ErrorHandlers don't have entry wrapping.
pub fn ready_error_handler_push(spec: &ErrorHandlerSpec) -> TokenStream {
    let role = &spec.role_variant;
    let trait_path = &spec.trait_path;
    quote! {
        __roles.push(#role(
            instance.clone() as ::std::sync::Arc<dyn #trait_path>
        ));
    }
}
