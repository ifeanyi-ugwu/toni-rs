//! Singleton Instance Injection Implementation
//!
//! Architecture:
//! 1. User struct with REAL fields (unchanged)
//! 2. `AppServiceProvider` (implements `Provider`) — holds `Arc<AppService>`, created once at startup
//! 3. `AppServiceProviderFactory` (implements `ProviderFactory`) — zero-sized descriptor; resolves
//!    deps and calls `build()` once to produce the `Provider` instance

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemImpl, ItemStruct, Result, Type};

use crate::{
    shared::{
        dependency_info::DependencyInfo,
        enhancer_markers::EnhancerMarkers,
        lifecycle_hooks::{
            LifecycleHooks, detect_lifecycle_hooks, reject_lifecycle_hooks, strip_lifecycle_attrs,
        },
        scope_parser::ProviderScope,
    },
    utils::extracts::{extract_vec_arc_dyn_inner, normalize_trait_send_sync},
};

/// Detected enhancer traits that a struct implements
#[derive(Debug, Clone, Default)]
pub struct EnhancerTraits {
    // Legacy enum-shaped roles (Guard / Interceptor / Pipe / ErrorHandler taking the
    // legacy `Context`). Set when the impl head has no generic arg (or `<Context>`)
    // or when the marker `#[guard]` etc. is present without transport args.
    pub is_guard: bool,
    pub is_interceptor: bool,
    pub is_pipe: bool,
    pub is_error_handler: bool,

    pub is_middleware: bool,
    pub is_gateway: bool,
    pub is_rpc_controller: bool,
    pub is_grpc_service: bool,

    // Per-transport typed roles. Multiple may be set on the same struct (e.g. a
    // guard with separate impl blocks for HttpContext and RpcContext, or a
    // universal blanket impl marked `#[guard(http, rpc, ws)]`).
    pub is_http_guard: bool,
    pub is_http_interceptor: bool,
    pub is_http_pipe: bool,
    pub is_http_error_handler: bool,

    pub is_rpc_guard: bool,
    pub is_rpc_interceptor: bool,
    pub is_rpc_pipe: bool,
    pub is_rpc_error_handler: bool,

    pub is_ws_guard: bool,
    pub is_ws_interceptor: bool,
    pub is_ws_pipe: bool,
    pub is_ws_error_handler: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct TransportFlags {
    http: bool,
    rpc: bool,
    ws: bool,
    /// Explicit `Context` or no generic arg — legacy path.
    legacy: bool,
    /// User declared a transport-specific intent (via marker arg or typed impl
    /// head). Suppresses the legacy fallback even if no transport matched.
    explicit: bool,
}

impl TransportFlags {
    fn any_typed(&self) -> bool {
        self.http || self.rpc || self.ws
    }

    fn merge(&mut self, other: TransportFlags) {
        self.http |= other.http;
        self.rpc |= other.rpc;
        self.ws |= other.ws;
        self.legacy |= other.legacy;
        self.explicit |= other.explicit;
    }
}

/// Parse `#[guard(http, rpc, ws)]` style transport args from a marker attribute.
///
/// Returns the union of transports requested; `explicit = true` if any args
/// were present (even unrecognized ones).
fn parse_marker_transport_args(attr: &syn::Attribute) -> TransportFlags {
    let mut flags = TransportFlags::default();
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return flags;
    }
    let parsed = attr.parse_args_with(
        syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated,
    );
    let Ok(idents) = parsed else {
        return flags;
    };
    for ident in idents {
        flags.explicit = true;
        match ident.to_string().as_str() {
            "http" => flags.http = true,
            "rpc" => flags.rpc = true,
            "ws" | "websocket" => flags.ws = true,
            "universal" | "all" => {
                flags.http = true;
                flags.rpc = true;
                flags.ws = true;
            }
            "context" | "legacy" => flags.legacy = true,
            _ => {}
        }
    }
    flags
}

/// Inspect a typed enhancer impl head (`Guard<...>` etc.) to decide which
/// transport(s) it serves. The first generic argument is the context type.
fn detect_typed_impl_transport(impl_block: &ItemImpl) -> TransportFlags {
    let mut flags = TransportFlags::default();
    let Some((_, path, _)) = &impl_block.trait_ else {
        return flags;
    };
    let Some(last) = path.segments.last() else {
        return flags;
    };

    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        // Bare `Guard` — defaults to `Context`. Legacy.
        return flags;
    };
    let Some(syn::GenericArgument::Type(ctx_ty)) = args.args.first() else {
        return flags;
    };
    let syn::Type::Path(type_path) = ctx_ty else {
        return flags;
    };
    let Some(last_seg) = type_path.path.segments.last() else {
        return flags;
    };

    flags.explicit = true;
    match last_seg.ident.to_string().as_str() {
        "HttpContext" => flags.http = true,
        "RpcContext" => flags.rpc = true,
        "WsContext" => flags.ws = true,
        "Context" => flags.legacy = true,
        // Generic type parameter like `C: HandlerContext + ?Sized` — universal.
        _ if impl_block
            .generics
            .params
            .iter()
            .any(|p| matches!(p, syn::GenericParam::Type(t) if t.ident == last_seg.ident)) =>
        {
            flags.http = true;
            flags.rpc = true;
            flags.ws = true;
        }
        // Unknown concrete type (e.g. user-defined HandlerContext impl).
        // Treat as explicit-but-unrouted; the legacy fallback won't fire.
        _ => {}
    }

    flags
}

/// Detect which enhancer traits a struct implements.
///
/// Checks marker attributes on the struct (`#[guard]`, `#[interceptor]`, etc.) and on the
/// impl block, as well as trait impl blocks for backwards compatibility.
fn detect_enhancer_traits(
    struct_def: Option<&ItemStruct>,
    impl_block: &ItemImpl,
) -> EnhancerTraits {
    let mut traits = EnhancerTraits::default();

    let struct_markers = struct_def.map(EnhancerMarkers::detect).unwrap_or_default();
    traits.is_middleware = struct_markers.is_middleware;

    // (transport_flags, signal_seen) — `signal_seen` gates whether the resolver
    // fires anything at all. Without a signal (no marker, no trait header match),
    // an unrelated struct (gateway, plain service) must not be cast as Guard.
    let mut guard = (TransportFlags::default(), false);
    let mut interceptor = (TransportFlags::default(), false);
    let mut pipe = (TransportFlags::default(), false);
    let mut error_handler = (TransportFlags::default(), false);

    let typed_impl_flags = detect_typed_impl_transport(impl_block);

    let merge_marker = |slot: &mut (TransportFlags, bool), attr: &syn::Attribute| {
        let mut f = parse_marker_transport_args(attr);
        if !f.explicit {
            f.merge(typed_impl_flags);
        }
        if !f.explicit {
            f.legacy = true;
        }
        slot.0.merge(f);
        slot.1 = true;
    };

    let scan_attrs = |attrs: &[syn::Attribute],
                      guard: &mut (TransportFlags, bool),
                      interceptor: &mut (TransportFlags, bool),
                      pipe: &mut (TransportFlags, bool),
                      error_handler: &mut (TransportFlags, bool),
                      traits: &mut EnhancerTraits| {
        for attr in attrs {
            let Some(ident) = attr.path().get_ident() else {
                continue;
            };
            match ident.to_string().as_str() {
                "guard" => merge_marker(guard, attr),
                "interceptor" => merge_marker(interceptor, attr),
                "pipe" => merge_marker(pipe, attr),
                "error_handler" => merge_marker(error_handler, attr),
                "middleware" => traits.is_middleware = true,
                _ => {}
            }
        }
    };

    if let Some(s) = struct_def {
        scan_attrs(
            &s.attrs,
            &mut guard,
            &mut interceptor,
            &mut pipe,
            &mut error_handler,
            &mut traits,
        );
    }
    scan_attrs(
        &impl_block.attrs,
        &mut guard,
        &mut interceptor,
        &mut pipe,
        &mut error_handler,
        &mut traits,
    );

    if let Some((_, path, _)) = &impl_block.trait_ {
        let trait_name = path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_default();

        match trait_name.as_str() {
            "Guard" => {
                guard.0.merge(typed_impl_flags);
                guard.1 = true;
            }
            "Interceptor" => {
                interceptor.0.merge(typed_impl_flags);
                interceptor.1 = true;
            }
            "Pipe" => {
                pipe.0.merge(typed_impl_flags);
                pipe.1 = true;
            }
            "ErrorHandler" => {
                error_handler.0.merge(typed_impl_flags);
                error_handler.1 = true;
            }
            "Middleware" => traits.is_middleware = true,
            _ => {}
        }
    }

    let resolve = |slot: (TransportFlags, bool)| -> (bool, bool, bool, bool) {
        if !slot.1 {
            return (false, false, false, false);
        }
        let f = slot.0;
        let legacy = f.legacy || (!f.any_typed() && !f.explicit);
        (legacy, f.http, f.rpc, f.ws)
    };

    let (g_l, g_h, g_r, g_w) = resolve(guard);
    traits.is_guard = g_l;
    traits.is_http_guard = g_h;
    traits.is_rpc_guard = g_r;
    traits.is_ws_guard = g_w;

    let (i_l, i_h, i_r, i_w) = resolve(interceptor);
    traits.is_interceptor = i_l;
    traits.is_http_interceptor = i_h;
    traits.is_rpc_interceptor = i_r;
    traits.is_ws_interceptor = i_w;

    let (p_l, p_h, p_r, p_w) = resolve(pipe);
    traits.is_pipe = p_l;
    traits.is_http_pipe = p_h;
    traits.is_rpc_pipe = p_r;
    traits.is_ws_pipe = p_w;

    let (e_l, e_h, e_r, e_w) = resolve(error_handler);
    traits.is_error_handler = e_l;
    traits.is_http_error_handler = e_h;
    traits.is_rpc_error_handler = e_r;
    traits.is_ws_error_handler = e_w;

    traits
}

/// Detect lifecycle hooks by scanning for method-level attributes in the impl block.
///
/// `struct_def` is `None` when the struct is defined separately above the impl block.
/// In that case the macro does not re-emit or modify the struct; the user owns it entirely
/// and must derive `Clone` themselves.
pub fn generate_instance_provider_system(
    struct_def: Option<&ItemStruct>,
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
    is_gateway: bool,
    is_rpc_controller: bool,
    is_grpc_service: bool,
) -> Result<TokenStream> {
    let struct_name = match struct_def {
        Some(s) => s.ident.clone(),
        None => crate::utils::extracts::extract_impl_self_ident(impl_block)?,
    };

    let struct_emit = struct_def.map(|s| {
        let s = add_clone_derive(s);
        quote! { #[allow(dead_code)] #s }
    });

    let mut impl_def = impl_block.clone();
    for item in impl_def.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            crate::markers_params::remove_marker_controller_fn::remove_marker_in_controller_fn_args(
                method,
            );
        }
    }
    let impl_def = strip_lifecycle_attrs(&impl_def);

    let mut enhancer_traits = detect_enhancer_traits(struct_def, impl_block);
    let lifecycle_hooks = detect_lifecycle_hooks(impl_block);
    enhancer_traits.is_gateway = is_gateway;
    enhancer_traits.is_rpc_controller = is_rpc_controller;
    enhancer_traits.is_grpc_service = is_grpc_service;

    let provider_wrapper = generate_provider_wrapper(
        &struct_name,
        dependencies,
        scope,
        &enhancer_traits,
        &lifecycle_hooks,
    );

    let factory = generate_factory(&struct_name, dependencies, scope, &enhancer_traits);
    let factory_accessor = generate_provider_factory_accessor(&struct_name);

    Ok(quote! {
        #struct_emit

        #[allow(dead_code)]
        #impl_def

        #provider_wrapper
        #factory
        #factory_accessor
    })
}

/// Adds Clone and Injectable derives to struct if needed
///
/// # Clone Detection
/// This function checks for `#[derive(Clone)]` attribute on the struct.
///
/// # Limitation: Manual `impl Clone`
/// This macro **cannot detect** manual `impl Clone` blocks that come after the macro invocation:
///
/// ```rust,ignore
/// #[injectable(pub struct Foo { field: String })]
/// impl Foo { /* ... */ }
///
/// // ❌ Macro cannot see this - will add #[derive(Clone)] and cause conflict
/// impl Clone for Foo {
///     fn clone(&self) -> Self { /* custom logic */ }
/// }
/// ```
///
/// This is an acceptable limitation because:
/// - Macros process attributes linearly and cannot look ahead to future impl blocks
/// - Compile errors are clear when conflicts occur
fn add_clone_derive(struct_attrs: &ItemStruct) -> ItemStruct {
    let mut struct_def = struct_attrs.clone();

    let has_clone = struct_def.attrs.iter().any(|attr| {
        if attr.path().is_ident("derive") {
            if let Ok(meta) = attr.parse_args::<syn::Meta>() {
                return meta_contains_clone(&meta);
            }
        }
        false
    });

    if !has_clone {
        // Add both Clone and Injectable derives
        // Injectable registers #[inject] and #[default] as valid attributes
        let derives: syn::Attribute = syn::parse_quote! {
            #[derive(Clone, ::toni::Injectable)]
        };
        struct_def.attrs.push(derives);
    } else {
        // Just add Injectable
        let injectable_derive: syn::Attribute = syn::parse_quote! {
            #[derive(::toni::Injectable)]
        };
        struct_def.attrs.push(injectable_derive);
    }

    struct_def
}

/// Recursively check if a derive meta contains Clone
fn meta_contains_clone(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("Clone"),
        syn::Meta::List(list) => {
            for nested in list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()
                .iter()
                .flatten()
            {
                if meta_contains_clone(nested) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn generate_provider_factory_accessor(struct_name: &Ident) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ProviderFactory", struct_name),
        struct_name.span(),
    );
    quote! {
        impl #struct_name {
            #[doc(hidden)]
            pub fn __toni_provider_factory() -> impl ::toni::traits_helpers::ProviderFactory {
                #factory_name
            }
        }
    }
}

fn generate_provider_wrapper(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
    enhancer_traits: &EnhancerTraits,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => generate_singleton_provider(struct_name, lifecycle_hooks),
        ProviderScope::Request => {
            generate_request_provider(struct_name, dependencies, enhancer_traits, lifecycle_hooks)
        }
        ProviderScope::Transient => {
            generate_transient_provider(struct_name, dependencies, enhancer_traits, lifecycle_hooks)
        }
    }
}

/// Generate role-push statements to embed inside `build()`, before the concrete
/// `instance: Arc<StructName>` is boxed. Returns a `TokenStream` that pushes
/// each role the struct implements onto a `__roles: Vec<ProviderRole>` local.
fn generate_role_pushes(traits: &EnhancerTraits) -> TokenStream {
    let mut pushes = Vec::new();

    if traits.is_guard {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Guard(
                ::toni::traits_helpers::GuardEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Guard>
                )
            ));
        });
    }
    if traits.is_interceptor {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Interceptor(
                ::toni::traits_helpers::InterceptorEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor>
                )
            ));
        });
    }
    if traits.is_pipe {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Pipe(
                ::toni::traits_helpers::PipeEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Pipe>
                )
            ));
        });
    }
    if traits.is_middleware {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Middleware(
                instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::middleware::Middleware>
            ));
        });
    }
    if traits.is_error_handler {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::ErrorHandler(
                instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::ErrorHandler>
            ));
        });
    }

    if traits.is_http_guard {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::HttpGuard(
                ::toni::traits_helpers::HttpGuardEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Guard<::toni::context::HttpContext>>
                )
            ));
        });
    }
    if traits.is_http_interceptor {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::HttpInterceptor(
                ::toni::traits_helpers::HttpInterceptorEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor<::toni::context::HttpContext>>
                )
            ));
        });
    }
    if traits.is_http_pipe {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::HttpPipe(
                ::toni::traits_helpers::HttpPipeEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Pipe<::toni::context::HttpContext>>
                )
            ));
        });
    }
    if traits.is_http_error_handler {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::HttpErrorHandler(
                instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::ErrorHandler<::toni::context::HttpContext, ::toni::http_helpers::HttpResponse>>
            ));
        });
    }

    if traits.is_rpc_guard {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::RpcGuard(
                ::toni::traits_helpers::RpcGuardEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Guard<::toni::context::RpcContext>>
                )
            ));
        });
    }
    if traits.is_rpc_interceptor {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::RpcInterceptor(
                ::toni::traits_helpers::RpcInterceptorEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor<::toni::context::RpcContext>>
                )
            ));
        });
    }
    if traits.is_rpc_pipe {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::RpcPipe(
                ::toni::traits_helpers::RpcPipeEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Pipe<::toni::context::RpcContext>>
                )
            ));
        });
    }
    if traits.is_rpc_error_handler {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::RpcErrorHandler(
                instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::ErrorHandler<::toni::context::RpcContext, ::toni::rpc::RpcData>>
            ));
        });
    }

    if traits.is_ws_guard {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::WsGuard(
                ::toni::traits_helpers::WsGuardEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Guard<::toni::context::WsContext>>
                )
            ));
        });
    }
    if traits.is_ws_interceptor {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::WsInterceptor(
                ::toni::traits_helpers::WsInterceptorEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor<::toni::context::WsContext>>
                )
            ));
        });
    }
    if traits.is_ws_pipe {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::WsPipe(
                ::toni::traits_helpers::WsPipeEntry::Ready(
                    instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::Pipe<::toni::context::WsContext>>
                )
            ));
        });
    }
    if traits.is_ws_error_handler {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::WsErrorHandler(
                instance.clone() as ::std::sync::Arc<dyn ::toni::traits_helpers::ErrorHandler<::toni::context::WsContext, ::toni::websocket::WsMessage>>
            ));
        });
    }
    if traits.is_gateway {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Gateway(
                ::std::sync::Arc::new(
                    Box::new((*instance).clone()) as Box<dyn ::toni::websocket::GatewayTrait>
                )
            ));
        });
    }
    if traits.is_rpc_controller {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::RpcController(
                ::std::sync::Arc::new(
                    Box::new((*instance).clone()) as Box<dyn ::toni::rpc::RpcControllerTrait>
                )
            ));
        });
    }
    if traits.is_grpc_service {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::GrpcService(
                ::std::sync::Arc::new(
                    Box::new((*instance).clone()) as Box<dyn ::toni::adapter::GrpcServiceTrait>
                )
            ));
        });
    }

    quote! { #(#pushes)* }
}

/// Generate direct lifecycle method overrides on `Provider` for singleton providers.
///
/// Each override delegates to the user's annotated method on `self.instance`.
/// Signal-bearing hooks receive the signal as the second argument.
fn generate_lifecycle_direct_methods(hooks: &LifecycleHooks) -> TokenStream {
    let mut methods = Vec::new();

    if let Some(method) = &hooks.on_module_init {
        methods.push(quote! {
            async fn on_module_init(&self) -> ::toni::InitResult {
                self.instance.#method().await
            }
        });
    }
    if let Some(method) = &hooks.on_application_bootstrap {
        methods.push(quote! {
            async fn on_application_bootstrap(&self) -> ::toni::InitResult {
                self.instance.#method().await
            }
        });
    }
    if let Some(method) = &hooks.on_module_destroy {
        methods.push(quote! {
            async fn on_module_destroy(&self) {
                self.instance.#method().await;
            }
        });
    }
    if let Some(method) = &hooks.before_application_shutdown {
        methods.push(quote! {
            async fn before_application_shutdown(&self, signal: Option<String>) {
                self.instance.#method(signal).await;
            }
        });
    }
    if let Some(method) = &hooks.on_application_shutdown {
        methods.push(quote! {
            async fn on_application_shutdown(&self, signal: Option<String>) {
                self.instance.#method(signal).await;
            }
        });
    }

    quote! { #(#methods)* }
}

fn generate_singleton_provider(
    struct_name: &Ident,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let lifecycle_methods = generate_lifecycle_direct_methods(lifecycle_hooks);

    quote! {
        struct #provider_name {
            instance: ::std::sync::Arc<#struct_name>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Provider for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _ctx: ::toni::ProviderContext<'_>,
            ) -> Box<dyn ::std::any::Any + Send> {
                Box::new((*self.instance).clone())
            }

            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_token_factory(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Singleton
            }

            #lifecycle_methods
        }
    }
}

fn generate_request_provider(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    _enhancer_traits: &EnhancerTraits,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    // Check if this uses from_request pattern
    let is_from_request = dependencies
        .init_method
        .as_ref()
        .map(|m| m == "from_request")
        .unwrap_or(false);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());

        if is_from_request {
            // Special case: from_request gets HttpRequest as first parameter.
            // __http_ctx is extracted at the top of the generated execute body.
            if field_names.is_empty() {
                // No dependencies, just the request parts
                quote! {
                    #struct_name::#init_ident(__http_ctx.parts)
                }
            } else {
                // Has dependencies + request parts
                quote! {
                    #struct_name::#init_ident(__http_ctx.parts, #(#field_names),*)
                }
            }
        } else {
            // Normal custom init
            quote! {
                #struct_name::#init_ident(#(#field_names),*)
            }
        }
    } else {
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    quote! { #field_name: #expr }
                } else {
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

    let scope_hook_error = reject_lifecycle_hooks(
        lifecycle_hooks,
        "Lifecycle hooks are not supported on request-scoped providers. Request-scoped \
         instances are created per-request and dropped when the response is sent — they do \
         not exist at application init or shutdown, so neither startup nor shutdown hooks \
         can fire. Use a singleton provider if you need lifecycle hooks.",
    );

    // Request-scoped providers require an active HTTP context. Constructing them
    // outside of a request would silently violate the declared scope contract.
    let execute_body = quote! {
        let ::toni::ProviderContext::Http(__http_ctx) = _ctx else {
            panic!(
                "Request-scoped provider '{}' requires an HTTP execution context. \
                 Request-scoped providers cannot be resolved outside of an active HTTP request.",
                ::std::any::type_name::<#struct_name>()
            );
        };
        if let Some(__cached) = __http_ctx.cache.get::<#struct_name>() {
            return Box::new(__cached);
        }
        #(#field_resolutions)*
        let instance = #struct_instantiation;
        __http_ctx.cache.insert(instance.clone());
        Box::new(instance)
    };

    quote! {
        #scope_hook_error

        struct #provider_name {
            dependencies: ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
            >,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Provider for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _ctx: ::toni::ProviderContext<'_>,
            ) -> Box<dyn ::std::any::Any + Send> {
                #execute_body
            }

            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_token_factory(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Request
            }
        }
    }
}

fn generate_transient_provider(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    _enhancer_traits: &EnhancerTraits,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        quote! {
            #struct_name::#init_ident(#(#field_names),*)
        }
    } else {
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    quote! { #field_name: #expr }
                } else {
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

    let scope_hook_error = reject_lifecycle_hooks(
        lifecycle_hooks,
        "Lifecycle hooks are not supported on transient-scoped providers. A transient's \
         lifetime is consumer-determined — singleton-shaped when consumed by a singleton, \
         request-shaped otherwise — so whether and when hooks fire depends on the consumer, \
         not the provider. Use a singleton provider if you need lifecycle hooks.",
    );

    quote! {
        #scope_hook_error

        struct #provider_name {
            dependencies: ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
            >,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Provider for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _ctx: ::toni::ProviderContext<'_>,
            ) -> Box<dyn ::std::any::Any + Send> {
                #(#field_resolutions)*

                let instance = #struct_instantiation;

                Box::new(instance)
            }

            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_token_factory(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Transient
            }
        }
    }
}

/// Generate field resolutions for Request/Transient providers (uses self.dependencies)
fn generate_field_resolutions(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    // When a constructor is specified, resolve its parameters instead of struct fields
    let deps_to_resolve = if !dependencies.constructor_params.is_empty() {
        &dependencies.constructor_params
    } else {
        &dependencies.fields
    };

    // Partition into multi-provider fields (Vec<Arc<dyn T>>) and regular fields
    let (multi_deps, regular_deps): (Vec<_>, Vec<_>) = deps_to_resolve
        .iter()
        .partition(|(_, full_type, _)| extract_vec_arc_dyn_inner(full_type).is_some());

    // Generate resolutions for multi-provider fields
    for (field_name, full_type, lookup_token_expr) in &multi_deps {
        let inner_trait = extract_vec_arc_dyn_inner(full_type).unwrap();
        // Downcast must use the normalized type (with + Send + Sync) since that is what
        // multi-providers store. The closure return type forces coercion back to inner_trait.
        let downcast_inner = normalize_trait_send_sync(inner_trait.clone());
        let field_name_str = field_name.to_string();
        let resolution = quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let provider = self.dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing multi-provider '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                let any_box = provider.execute(vec![], _ctx).await;
                let erased_items = *any_box
                    .downcast::<Vec<::std::sync::Arc<dyn ::std::any::Any + Send + Sync>>>()
                    .unwrap_or_else(|_| panic!(
                        "Multi-provider '{}' returned unexpected type (expected Vec<Arc<dyn Any+Send+Sync>>)",
                        __lookup_token
                    ));
                erased_items
                    .into_iter()
                    .map(|item| -> ::std::sync::Arc<#inner_trait> {
                        let wrapped = ::std::sync::Arc::downcast::<::std::sync::Arc<#downcast_inner>>(item)
                            .unwrap_or_else(|_| panic!(
                                "Multi-provider '{}': item downcast to Arc<{}> failed",
                                __lookup_token,
                                stringify!(#downcast_inner)
                            ));
                        (*wrapped).clone()
                    })
                    .collect()
            };
        };
        resolutions.push(resolution);
        field_names.push(field_name.clone());
    }

    // Group regular fields by token for deduplication while preserving declaration order
    use indexmap::IndexMap;
    let mut type_groups: IndexMap<String, Vec<(Ident, Type, TokenStream)>> = IndexMap::new();

    for (field_name, full_type, lookup_token_expr) in &regular_deps {
        let type_key = quote!(#lookup_token_expr).to_string();
        type_groups.entry(type_key).or_insert_with(Vec::new).push((
            (*field_name).clone(),
            (*full_type).clone(),
            (*lookup_token_expr).clone(),
        ));
    }
    for (_type_key, fields_of_type) in type_groups {
        let (first_field_name, full_type, lookup_token_expr) = &fields_of_type[0];
        let field_name_str = first_field_name.to_string();

        if fields_of_type.len() == 1 {
            let field_name = first_field_name;
            let resolution = quote! {
                let #field_name: #full_type = {
                    let __lookup_token = #lookup_token_expr;
                    let provider = self.dependencies
                        .get(&__lookup_token)
                        .unwrap_or_else(|| panic!(
                            "Missing dependency '{}' for field '{}'",
                            __lookup_token, #field_name_str
                        ));

                    let any_box = provider.execute(vec![], _ctx).await;

                    *any_box.downcast::<#full_type>()
                        .unwrap_or_else(|_| panic!(
                            "Failed to downcast '{}' to {}",
                            __lookup_token,
                            stringify!(#full_type)
                        ))
                };
            };

            resolutions.push(resolution);
            field_names.push(field_name.clone());
        } else {
            let temp_var = syn::Ident::new(
                &format!("__temp_instance_{}", first_field_name),
                first_field_name.span(),
            );
            let field_idents: Vec<_> = fields_of_type.iter().map(|(name, _, _)| name).collect();

            let field_declarations: Vec<TokenStream> = field_idents
                .iter()
                .map(|field_ident| {
                    quote! {
                        let #field_ident: #full_type;
                    }
                })
                .collect();

            let resolution = quote! {
                #(#field_declarations)*

                let __lookup_token = #lookup_token_expr;
                let provider = self.dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));

                if matches!(provider.get_scope(), ::toni::ProviderScope::Transient) {
                    #(
                        #field_idents = {
                            let any_box = provider.execute(vec![], _ctx).await;
                            *any_box.downcast::<#full_type>()
                                .unwrap_or_else(|_| panic!(
                                    "Failed to downcast '{}' to {}",
                                    __lookup_token,
                                    stringify!(#full_type)
                                ))
                        };
                    )*
                } else {
                    let #temp_var: #full_type = {
                        let any_box = provider.execute(vec![], _ctx).await;
                        *any_box.downcast::<#full_type>()
                            .unwrap_or_else(|_| panic!(
                                "Failed to downcast '{}' to {}",
                                __lookup_token,
                                stringify!(#full_type)
                            ))
                    };

                    #(
                        #field_idents = #temp_var.clone();
                    )*
                }
            };

            resolutions.push(resolution);
            for (field_name, _, _) in &fields_of_type {
                field_names.push(field_name.clone());
            }
        }
    }

    (resolutions, field_names)
}

/// Generate field resolutions for singleton factory (uses dependencies parameter)
fn generate_factory_field_resolutions(
    dependencies: &DependencyInfo,
) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    // When a constructor is specified, resolve its parameters instead of struct fields
    let deps_to_resolve = if !dependencies.constructor_params.is_empty() {
        &dependencies.constructor_params
    } else {
        &dependencies.fields
    };

    // Partition into multi-provider fields (Vec<Arc<dyn T>>) and regular fields
    let (multi_deps, regular_deps): (Vec<_>, Vec<_>) = deps_to_resolve
        .iter()
        .partition(|(_, full_type, _)| extract_vec_arc_dyn_inner(full_type).is_some());

    // Generate resolutions for multi-provider fields
    for (field_name, full_type, lookup_token_expr) in &multi_deps {
        let inner_trait = extract_vec_arc_dyn_inner(full_type).unwrap();
        let downcast_inner = normalize_trait_send_sync(inner_trait.clone());
        let field_name_str = field_name.to_string();
        let resolution = quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let provider = dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing multi-provider '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                let any_box = provider.execute(vec![], ::toni::ProviderContext::None).await;
                let erased_items = *any_box
                    .downcast::<Vec<::std::sync::Arc<dyn ::std::any::Any + Send + Sync>>>()
                    .unwrap_or_else(|_| panic!(
                        "Multi-provider '{}' returned unexpected type (expected Vec<Arc<dyn Any+Send+Sync>>)",
                        __lookup_token
                    ));
                erased_items
                    .into_iter()
                    .map(|item| -> ::std::sync::Arc<#inner_trait> {
                        let wrapped = ::std::sync::Arc::downcast::<::std::sync::Arc<#downcast_inner>>(item)
                            .unwrap_or_else(|_| panic!(
                                "Multi-provider '{}': item downcast to Arc<{}> failed",
                                __lookup_token,
                                stringify!(#downcast_inner)
                            ));
                        (*wrapped).clone()
                    })
                    .collect()
            };
        };
        resolutions.push(resolution);
        field_names.push(field_name.clone());
    }

    // Group regular fields by token for deduplication while preserving declaration order
    use indexmap::IndexMap;
    let mut type_groups: IndexMap<String, Vec<(Ident, Type, TokenStream)>> = IndexMap::new();

    for (field_name, full_type, lookup_token_expr) in &regular_deps {
        let type_key = quote!(#lookup_token_expr).to_string();
        type_groups.entry(type_key).or_insert_with(Vec::new).push((
            (*field_name).clone(),
            (*full_type).clone(),
            (*lookup_token_expr).clone(),
        ));
    }
    for (_type_key, fields_of_type) in type_groups {
        let (first_field_name, full_type, lookup_token_expr) = &fields_of_type[0];
        let field_name_str = first_field_name.to_string();

        if fields_of_type.len() == 1 {
            let field_name = first_field_name;
            let resolution = quote! {
                let #field_name: #full_type = {
                    let __lookup_token = #lookup_token_expr;
                    let provider = dependencies
                        .get(&__lookup_token)
                        .unwrap_or_else(|| panic!(
                            "Missing dependency '{}' for field '{}'",
                            __lookup_token, #field_name_str
                        ));

                    let any_box = provider.execute(vec![], ::toni::ProviderContext::None).await;

                    *any_box.downcast::<#full_type>()
                        .unwrap_or_else(|_| panic!(
                            "Failed to downcast '{}' to {}",
                            __lookup_token,
                            stringify!(#full_type)
                        ))
                };
            };

            resolutions.push(resolution);
            field_names.push(field_name.clone());
        } else {
            let temp_var = syn::Ident::new(
                &format!("__temp_instance_{}", first_field_name),
                first_field_name.span(),
            );
            let field_idents: Vec<_> = fields_of_type.iter().map(|(name, _, _)| name).collect();

            let field_declarations: Vec<TokenStream> = field_idents
                .iter()
                .map(|field_ident| {
                    quote! {
                        let #field_ident: #full_type;
                    }
                })
                .collect();

            let resolution = quote! {
                #(#field_declarations)*

                let __lookup_token = #lookup_token_expr;
                let provider = dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));

                if matches!(provider.get_scope(), ::toni::ProviderScope::Transient) {
                    #(
                        #field_idents = {
                            let any_box = provider.execute(vec![], ::toni::ProviderContext::None).await;
                            *any_box.downcast::<#full_type>()
                                .unwrap_or_else(|_| panic!(
                                    "Failed to downcast '{}' to {}",
                                    __lookup_token,
                                    stringify!(#full_type)
                                ))
                        };
                    )*
                } else {
                    let #temp_var: #full_type = {
                        let any_box = provider.execute(vec![], ::toni::ProviderContext::None).await;
                        *any_box.downcast::<#full_type>()
                            .unwrap_or_else(|_| panic!(
                                "Failed to downcast '{}' to {}",
                                __lookup_token,
                                stringify!(#full_type)
                            ))
                    };

                    #(
                        #field_idents = #temp_var.clone();
                    )*
                }
            };

            resolutions.push(resolution);
            for (field_name, _, _) in &fields_of_type {
                field_names.push(field_name.clone());
            }
        }
    }

    (resolutions, field_names)
}

fn generate_factory(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
    enhancer_traits: &EnhancerTraits,
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => {
            generate_singleton_factory(struct_name, dependencies, enhancer_traits)
        }
        ProviderScope::Request => {
            generate_request_factory(struct_name, dependencies, enhancer_traits)
        }
        ProviderScope::Transient => {
            generate_transient_factory(struct_name, dependencies, enhancer_traits)
        }
    }
}

fn generate_singleton_factory(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    enhancer_traits: &EnhancerTraits,
) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ProviderFactory", struct_name),
        struct_name.span(),
    );
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let (field_resolutions, field_names) = generate_factory_field_resolutions(dependencies);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        // Custom init method: MyService::new(dep1, dep2, ...)
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        quote! {
            #struct_name::#init_ident(#(#field_names),*)
        }
    } else {
        // Standard struct literal: MyService { dep1, dep2, field3: default, ... }
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    // User provided #[default(...)]
                    quote! { #field_name: #expr }
                } else {
                    // Fall back to Default trait
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

    // Collect dependency tokens from both constructor params (if using constructor injection)
    // and from #[inject] fields (if using field injection)
    let dependency_tokens: Vec<_> = dependencies
        .constructor_params
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .chain(
            dependencies
                .fields
                .iter()
                .map(|(_, _, lookup_token_expr)| lookup_token_expr),
        )
        .collect();

    // Generate scope validation code (Singleton cannot inject Request)
    // Check both constructor params and #[inject] fields
    let has_dependencies =
        !dependencies.constructor_params.is_empty() || !dependencies.fields.is_empty();
    let scope_validation = if has_dependencies {
        // Combine constructor params and fields for validation
        let constructor_dep_checks = dependencies.constructor_params.iter().map(
            |(param_name, _param_type, lookup_token_expr)| {
                let param_str = param_name.to_string();
                (
                    param_str,
                    quote! { "constructor parameter" },
                    lookup_token_expr,
                )
            },
        );

        let field_dep_checks =
            dependencies
                .fields
                .iter()
                .map(|(field_name, _full_type, lookup_token_expr)| {
                    let field_str = field_name.to_string();
                    (field_str, quote! { "field" }, lookup_token_expr)
                });

        let dep_checks: Vec<_> = constructor_dep_checks
            .chain(field_dep_checks)
            .map(|(dep_name, _dep_kind, lookup_token_expr)| {
                quote! {
                    {
                        let __lookup_token = #lookup_token_expr;
                        if let Some(provider) = dependencies.get(&__lookup_token) {
                            let dep_scope = provider.get_scope();
                            if matches!(dep_scope, ::toni::ProviderScope::Request) {
                                panic!(
                                    "\n❌ Scope validation error in provider '{}':\n\
                                     \n\
                                     Singleton-scoped providers cannot inject Request-scoped providers.\n\
                                     Dependency '{}' depends on '{}' which has Request scope.\n\
                                     \n\
                                     This restriction prevents data leakage across requests. Singleton providers\n\
                                     live for the entire application lifetime and would capture stale request data.\n\
                                     \n\
                                     Solutions:\n\
                                     1. Change '{}' to Request scope: #[injectable(scope = \"request\")]\n\
                                     2. Change '{}' to Singleton scope (if appropriate for your use case)\n\
                                     3. Pass request-specific data as method parameters instead of injecting\n\
                                     4. Extract data in controller (which has HttpRequest access) and pass it down\n\
                                     \n",
                                    ::std::any::type_name::<#struct_name>(),
                                    #dep_name,
                                    __lookup_token,
                                    ::std::any::type_name::<#struct_name>(),
                                    __lookup_token
                                );
                            }
                        }
                    }
                }
            })
            .collect();

        quote! {
            // Validate scope compatibility (runtime check at startup)
            #(#dep_checks)*
        }
    } else {
        quote! {}
    };

    let role_pushes = generate_role_pushes(enhancer_traits);

    quote! {
        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ProviderFactory for #factory_name {
            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#dependency_tokens),*]
            }

            async fn build(
                &self,
                __deps: ::toni::FxHashMap<String, ::toni::traits_helpers::Injectable>,
            ) -> ::toni::traits_helpers::Injectable {
                let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                    __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();

                #scope_validation

                // Resolve all dependencies at startup
                #(#field_resolutions)*

                let instance = ::std::sync::Arc::new({
                    #struct_instantiation
                });

                let mut __roles = ::std::vec::Vec::new();
                #role_pushes

                let provider = ::std::sync::Arc::new(Box::new(#provider_name { instance }) as Box<dyn ::toni::traits_helpers::Provider>);
                ::toni::traits_helpers::Injectable::new(provider, __roles)
            }
        }
    }
}

fn generate_request_factory(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    enhancer_traits: &EnhancerTraits,
) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ProviderFactory", struct_name),
        struct_name.span(),
    );
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let dependency_tokens: Vec<_> = dependencies
        .constructor_params
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .chain(
            dependencies
                .fields
                .iter()
                .map(|(_, _, lookup_token_expr)| lookup_token_expr),
        )
        .collect();

    let (dyn_factory_structs, factory_role_pushes) =
        generate_dyn_factories(struct_name, dependencies, enhancer_traits);

    let has_enhancer_roles = !factory_role_pushes.is_empty();

    let build_body = if has_enhancer_roles {
        quote! {
            let __has_request_deps = __deps.values().any(|inj|
                matches!(inj.instance.get_scope(), ::toni::ProviderScope::Request)
            );
            let __all_deps = ::std::sync::Arc::new(
                __deps.iter()
                    .map(|(k, inj)| (k.clone(), inj.instance.clone()))
                    .collect::<::toni::FxHashMap<_, _>>()
            );
            let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
            let mut __roles = ::std::vec::Vec::new();
            #factory_role_pushes
            ::toni::traits_helpers::Injectable::new(
                ::std::sync::Arc::new(Box::new(#provider_name { dependencies }) as Box<dyn ::toni::traits_helpers::Provider>),
                __roles,
            )
        }
    } else {
        quote! {
            let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
            ::toni::traits_helpers::Injectable::new(
                ::std::sync::Arc::new(Box::new(#provider_name { dependencies }) as Box<dyn ::toni::traits_helpers::Provider>),
                ::std::vec::Vec::new(),
            )
        }
    };

    quote! {
        #dyn_factory_structs

        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ProviderFactory for #factory_name {
            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#dependency_tokens),*]
            }

            async fn build(
                &self,
                __deps: ::toni::FxHashMap<String, ::toni::traits_helpers::Injectable>,
            ) -> ::toni::traits_helpers::Injectable {
                #build_body
            }
        }
    }
}

fn generate_transient_factory(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    enhancer_traits: &EnhancerTraits,
) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ProviderFactory", struct_name),
        struct_name.span(),
    );
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let dependency_tokens: Vec<_> = dependencies
        .constructor_params
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .chain(
            dependencies
                .fields
                .iter()
                .map(|(_, _, lookup_token_expr)| lookup_token_expr),
        )
        .collect();

    let (dyn_factory_structs, factory_role_pushes) =
        generate_dyn_factories(struct_name, dependencies, enhancer_traits);

    let has_enhancer_roles = !factory_role_pushes.is_empty();

    let build_body = if has_enhancer_roles {
        quote! {
            let __has_request_deps = __deps.values().any(|inj|
                matches!(inj.instance.get_scope(), ::toni::ProviderScope::Request)
            );
            let __all_deps = ::std::sync::Arc::new(
                __deps.iter()
                    .map(|(k, inj)| (k.clone(), inj.instance.clone()))
                    .collect::<::toni::FxHashMap<_, _>>()
            );
            let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
            let mut __roles = ::std::vec::Vec::new();
            #factory_role_pushes
            ::toni::traits_helpers::Injectable::new(
                ::std::sync::Arc::new(Box::new(#provider_name { dependencies }) as Box<dyn ::toni::traits_helpers::Provider>),
                __roles,
            )
        }
    } else {
        quote! {
            let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
            ::toni::traits_helpers::Injectable::new(
                ::std::sync::Arc::new(Box::new(#provider_name { dependencies }) as Box<dyn ::toni::traits_helpers::Provider>),
                ::std::vec::Vec::new(),
            )
        }
    };

    quote! {
        #dyn_factory_structs

        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ProviderFactory for #factory_name {
            fn get_token(&self) -> String {
                ::std::any::type_name::<#struct_name>().to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#dependency_tokens),*]
            }

            async fn build(
                &self,
                __deps: ::toni::FxHashMap<String, ::toni::traits_helpers::Injectable>,
            ) -> ::toni::traits_helpers::Injectable {
                #build_body
            }
        }
    }
}

/// Generates the dep resolution code for use inside a `DynXxxFactory::create()` body.
///
/// Unlike `generate_field_resolutions` (which uses `self.dependencies` and `_ctx`),
/// this version uses a captured `all_deps: Arc<FxHashMap<...>>` and selects the
/// `ProviderContext` at runtime based on each provider's declared scope.
fn generate_create_field_resolutions(
    dependencies: &DependencyInfo,
) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    let deps_to_resolve = if !dependencies.constructor_params.is_empty() {
        &dependencies.constructor_params
    } else {
        &dependencies.fields
    };

    let (multi_deps, regular_deps): (Vec<_>, Vec<_>) = deps_to_resolve
        .iter()
        .partition(|(_, full_type, _)| extract_vec_arc_dyn_inner(full_type).is_some());

    for (field_name, full_type, lookup_token_expr) in &multi_deps {
        let inner_trait = extract_vec_arc_dyn_inner(full_type).unwrap();
        let downcast_inner = normalize_trait_send_sync(inner_trait.clone());
        let field_name_str = field_name.to_string();
        resolutions.push(quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let __provider = all_deps.get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing multi-provider '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                let __ctx = if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                    ::toni::ProviderContext::Http(::toni::traits_helpers::HttpContext {
                        parts: request_parts.expect("HTTP request context required for request-scoped dependency"),
                        cache: &__request_cache,
                    })
                } else {
                    ::toni::ProviderContext::None
                };
                let __any_box = __provider.execute(::std::vec::Vec::new(), __ctx).await;
                let erased_items = *__any_box
                    .downcast::<Vec<::std::sync::Arc<dyn ::std::any::Any + Send + Sync>>>()
                    .unwrap_or_else(|_| panic!(
                        "Multi-provider '{}' returned unexpected type (expected Vec<Arc<dyn Any+Send+Sync>>)",
                        __lookup_token
                    ));
                erased_items
                    .into_iter()
                    .map(|item| -> ::std::sync::Arc<#inner_trait> {
                        let wrapped = ::std::sync::Arc::downcast::<::std::sync::Arc<#downcast_inner>>(item)
                            .unwrap_or_else(|_| panic!(
                                "Multi-provider '{}': item downcast to Arc<{}> failed",
                                __lookup_token,
                                stringify!(#downcast_inner)
                            ));
                        (*wrapped).clone()
                    })
                    .collect()
            };
        });
        field_names.push(field_name.clone());
    }

    for (field_name, full_type, lookup_token_expr) in &regular_deps {
        let field_name_str = field_name.to_string();
        resolutions.push(quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let __provider = all_deps.get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                let __ctx = if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                    ::toni::ProviderContext::Http(::toni::traits_helpers::HttpContext {
                        parts: request_parts.expect("HTTP request context required for request-scoped dependency"),
                        cache: &__request_cache,
                    })
                } else {
                    ::toni::ProviderContext::None
                };
                let __any_box = __provider.execute(::std::vec::Vec::new(), __ctx).await;
                *__any_box.downcast::<#full_type>()
                    .unwrap_or_else(|_| panic!(
                        "Failed to downcast '{}' to {}",
                        __lookup_token,
                        stringify!(#full_type)
                    ))
            };
        });
        field_names.push(field_name.clone());
    }

    (resolutions, field_names)
}

/// Generates the `DynGuardFactory`, `DynInterceptorFactory`, and/or `DynPipeFactory`
/// implementor structs for request/transient-scoped providers that are also enhancers.
///
/// Returns `(struct_defs, role_pushes)`:
/// - `struct_defs`: emitted before the provider factory struct
/// - `role_pushes`: emitted inside `build()`, assumes `__all_deps` and `__has_request_deps` are in scope
fn generate_dyn_factories(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    enhancer_traits: &EnhancerTraits,
) -> (TokenStream, TokenStream) {
    let any_factory = enhancer_traits.is_guard
        || enhancer_traits.is_interceptor
        || enhancer_traits.is_pipe
        || enhancer_traits.is_http_guard
        || enhancer_traits.is_http_interceptor
        || enhancer_traits.is_http_pipe
        || enhancer_traits.is_rpc_guard
        || enhancer_traits.is_rpc_interceptor
        || enhancer_traits.is_rpc_pipe
        || enhancer_traits.is_ws_guard
        || enhancer_traits.is_ws_interceptor
        || enhancer_traits.is_ws_pipe;

    if !any_factory {
        return (quote! {}, quote! {});
    }

    let (field_resolutions, field_names) = generate_create_field_resolutions(dependencies);

    // Struct construction — same shape as request/transient provider execute()
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        let is_from_request = init_fn == "from_request";
        if is_from_request {
            if field_names.is_empty() {
                quote! { #struct_name::#init_ident(request_parts.expect("HTTP request context required")) }
            } else {
                quote! { #struct_name::#init_ident(request_parts.expect("HTTP request context required"), #(#field_names),*) }
            }
        } else {
            quote! { #struct_name::#init_ident(#(#field_names),*) }
        }
    } else {
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    quote! { #field_name: #expr }
                } else {
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();
        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

    let deps_arc_ty = quote! {
        ::std::sync::Arc<::toni::FxHashMap<
            String,
            ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
        >>
    };

    let mut struct_defs = Vec::new();
    let mut role_push_stmts = Vec::new();

    let mut emit = |kind_name: &str,
                    trait_path: TokenStream,
                    factory_trait_path: TokenStream,
                    role_variant: TokenStream,
                    entry_variant: TokenStream| {
        let factory_struct_name = Ident::new(
            &format!("__Toni{}{}DynFactory", struct_name, kind_name),
            struct_name.span(),
        );
        struct_defs.push(quote! {
            struct #factory_struct_name {
                all_deps: #deps_arc_ty,
                has_request_deps: bool,
            }

            impl #factory_trait_path for #factory_struct_name {
                fn requires_http_parts(&self) -> bool {
                    self.has_request_deps
                }

                fn create<'a>(
                    &'a self,
                    request_parts: Option<&'a ::toni::http_helpers::RequestPart>,
                ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<
                    Output = ::std::sync::Arc<dyn #trait_path + Send + Sync>
                > + Send + 'a>> {
                    let all_deps = self.all_deps.clone();
                    ::std::boxed::Box::pin(async move {
                        let __request_cache = ::toni::traits_helpers::RequestCache::new();
                        #(#field_resolutions)*
                        let instance = #struct_instantiation;
                        ::std::sync::Arc::new(instance) as ::std::sync::Arc<dyn #trait_path + Send + Sync>
                    })
                }
            }
        });
        role_push_stmts.push(quote! {
            __roles.push(#role_variant(
                #entry_variant(
                    ::std::sync::Arc::new(#factory_struct_name {
                        all_deps: __all_deps.clone(),
                        has_request_deps: __has_request_deps,
                    })
                )
            ));
        });
    };

    if enhancer_traits.is_guard {
        emit(
            "Guard",
            quote! { ::toni::traits_helpers::Guard },
            quote! { ::toni::traits_helpers::DynGuardFactory },
            quote! { ::toni::traits_helpers::ProviderRole::Guard },
            quote! { ::toni::traits_helpers::GuardEntry::Factory },
        );
    }
    if enhancer_traits.is_interceptor {
        emit(
            "Interceptor",
            quote! { ::toni::traits_helpers::Interceptor },
            quote! { ::toni::traits_helpers::DynInterceptorFactory },
            quote! { ::toni::traits_helpers::ProviderRole::Interceptor },
            quote! { ::toni::traits_helpers::InterceptorEntry::Factory },
        );
    }
    if enhancer_traits.is_pipe {
        emit(
            "Pipe",
            quote! { ::toni::traits_helpers::Pipe },
            quote! { ::toni::traits_helpers::DynPipeFactory },
            quote! { ::toni::traits_helpers::ProviderRole::Pipe },
            quote! { ::toni::traits_helpers::PipeEntry::Factory },
        );
    }

    if enhancer_traits.is_http_guard {
        emit(
            "HttpGuard",
            quote! { ::toni::traits_helpers::Guard<::toni::context::HttpContext> },
            quote! { ::toni::traits_helpers::DynHttpGuardFactory },
            quote! { ::toni::traits_helpers::ProviderRole::HttpGuard },
            quote! { ::toni::traits_helpers::HttpGuardEntry::Factory },
        );
    }
    if enhancer_traits.is_http_interceptor {
        emit(
            "HttpInterceptor",
            quote! { ::toni::traits_helpers::Interceptor<::toni::context::HttpContext> },
            quote! { ::toni::traits_helpers::DynHttpInterceptorFactory },
            quote! { ::toni::traits_helpers::ProviderRole::HttpInterceptor },
            quote! { ::toni::traits_helpers::HttpInterceptorEntry::Factory },
        );
    }
    if enhancer_traits.is_http_pipe {
        emit(
            "HttpPipe",
            quote! { ::toni::traits_helpers::Pipe<::toni::context::HttpContext> },
            quote! { ::toni::traits_helpers::DynHttpPipeFactory },
            quote! { ::toni::traits_helpers::ProviderRole::HttpPipe },
            quote! { ::toni::traits_helpers::HttpPipeEntry::Factory },
        );
    }

    if enhancer_traits.is_rpc_guard {
        emit(
            "RpcGuard",
            quote! { ::toni::traits_helpers::Guard<::toni::context::RpcContext> },
            quote! { ::toni::traits_helpers::DynRpcGuardFactory },
            quote! { ::toni::traits_helpers::ProviderRole::RpcGuard },
            quote! { ::toni::traits_helpers::RpcGuardEntry::Factory },
        );
    }
    if enhancer_traits.is_rpc_interceptor {
        emit(
            "RpcInterceptor",
            quote! { ::toni::traits_helpers::Interceptor<::toni::context::RpcContext> },
            quote! { ::toni::traits_helpers::DynRpcInterceptorFactory },
            quote! { ::toni::traits_helpers::ProviderRole::RpcInterceptor },
            quote! { ::toni::traits_helpers::RpcInterceptorEntry::Factory },
        );
    }
    if enhancer_traits.is_rpc_pipe {
        emit(
            "RpcPipe",
            quote! { ::toni::traits_helpers::Pipe<::toni::context::RpcContext> },
            quote! { ::toni::traits_helpers::DynRpcPipeFactory },
            quote! { ::toni::traits_helpers::ProviderRole::RpcPipe },
            quote! { ::toni::traits_helpers::RpcPipeEntry::Factory },
        );
    }

    if enhancer_traits.is_ws_guard {
        emit(
            "WsGuard",
            quote! { ::toni::traits_helpers::Guard<::toni::context::WsContext> },
            quote! { ::toni::traits_helpers::DynWsGuardFactory },
            quote! { ::toni::traits_helpers::ProviderRole::WsGuard },
            quote! { ::toni::traits_helpers::WsGuardEntry::Factory },
        );
    }
    if enhancer_traits.is_ws_interceptor {
        emit(
            "WsInterceptor",
            quote! { ::toni::traits_helpers::Interceptor<::toni::context::WsContext> },
            quote! { ::toni::traits_helpers::DynWsInterceptorFactory },
            quote! { ::toni::traits_helpers::ProviderRole::WsInterceptor },
            quote! { ::toni::traits_helpers::WsInterceptorEntry::Factory },
        );
    }
    if enhancer_traits.is_ws_pipe {
        emit(
            "WsPipe",
            quote! { ::toni::traits_helpers::Pipe<::toni::context::WsContext> },
            quote! { ::toni::traits_helpers::DynWsPipeFactory },
            quote! { ::toni::traits_helpers::ProviderRole::WsPipe },
            quote! { ::toni::traits_helpers::WsPipeEntry::Factory },
        );
    }

    (quote! { #(#struct_defs)* }, quote! { #(#role_push_stmts)* })
}
