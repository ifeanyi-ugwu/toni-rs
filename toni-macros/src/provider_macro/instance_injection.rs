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
        lifecycle_hooks::{
            LifecycleHooks, detect_lifecycle_hooks, reject_lifecycle_hooks, strip_lifecycle_attrs,
        },
        scope_parser::ProviderScope,
    },
    utils::extracts::{
        extract_struct_dependencies, extract_vec_arc_dyn_inner, normalize_trait_send_sync,
    },
};

/// Structural roles the surrounding macro assigns to a provider.
///
/// Enhancer roles (guard / interceptor / error-handler / middleware) are NOT here — those
/// are detected from the type's trait impls via `toni::__detect` at the factory. These three are
/// driven by the structural macros (`#[websocket_gateway]` / `#[rpc_controller]` / `#[grpc_service]`),
/// which generate the corresponding trait impl and routing, so the macro that emits them already
/// knows the role.
#[derive(Debug, Clone, Default)]
pub struct EnhancerTraits {
    pub is_gateway: bool,
}

/// The DI wiring for a `#[grpc_service]`: the re-emitted struct and impl, the lifecycle forwarders
/// scanned off that impl, and the controller system its declaration expands to.
///
/// `struct_def` is `None` when the struct is defined separately above the impl block.
/// In that case the macro does not re-emit or modify the struct; the user owns it entirely
/// and must derive `Clone` themselves.
pub fn generate_grpc_service_wiring(
    struct_def: Option<&ItemStruct>,
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    request_scoped: bool,
) -> Result<TokenStream> {
    let struct_name = match struct_def {
        Some(s) => s.ident.clone(),
        None => crate::utils::extracts::extract_impl_self_ident(impl_block)?,
    };

    let struct_emit = struct_def.map(|s| {
        let s = add_clone_and_inject_fields(s);
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

    let lifecycle_hooks = detect_lifecycle_hooks(impl_block);

    // A dispatch target can be built per call, and the per-call path has no `Provider` of its own to
    // hang hooks on — it reaches them through the bridge, like a request-scoped HTTP controller. So
    // the scanned hooks are re-emitted as the inherent forwarders the bridge dispatches to, and the
    // controller object calls the same names.
    let lifecycle_forwarders = generate_lifecycle_forwarders(&struct_name, &lifecycle_hooks);
    let controller_system =
        generate_grpc_service_system(&struct_name, dependencies, request_scoped);

    Ok(quote! {
        #struct_emit

        #[allow(dead_code)]
        #impl_def

        #lifecycle_forwarders
        #controller_system
    })
}

/// Re-emit scanned lifecycle hooks as the inherent `__toni_lc_*` forwarders that shadow the blanket
/// `LifecycleBridge` no-ops — the same shape `#[on_module_init]` and friends emit for `#[injectable]`
/// structs, which never run here because the enclosing attribute consumes them. A hook the struct
/// does not declare is left to the blanket default.
fn generate_lifecycle_forwarders(struct_name: &Ident, hooks: &LifecycleHooks) -> TokenStream {
    let mut methods = Vec::new();

    if let Some(method) = &hooks.on_module_init {
        methods.push(quote! {
            async fn __toni_lc_on_init(&self) -> ::toni::InitResult { self.#method().await }
        });
    }
    if let Some(method) = &hooks.on_application_bootstrap {
        methods.push(quote! {
            async fn __toni_lc_on_bootstrap(&self) -> ::toni::InitResult { self.#method().await }
        });
    }
    if let Some(method) = &hooks.on_module_destroy {
        methods.push(quote! {
            async fn __toni_lc_on_destroy(&self) { self.#method().await; }
        });
    }
    if let Some(method) = &hooks.before_application_shutdown {
        methods.push(quote! {
            async fn __toni_lc_before_shutdown(&self, signal: Option<String>) {
                self.#method(signal).await;
            }
        });
    }
    if let Some(method) = &hooks.on_application_shutdown {
        methods.push(quote! {
            async fn __toni_lc_on_shutdown(&self, signal: Option<String>) {
                self.#method(signal).await;
            }
        });
    }

    if methods.is_empty() {
        return quote! {};
    }

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        impl #struct_name {
            #(#methods)*
        }
    }
}

/// Emit the provider + factory + accessor for `struct_def` — the `#[injectable]` codegen. The caller
/// (`provider_attr`) re-emits the struct itself; this contributes only the DI wiring beside it.
///
/// Dependencies are declared as `#[inject]` fields. By default the instance is assembled as
/// a struct literal (`#[default(...)]` for owned state). With `init = "new"`, construction is
/// redirected through `Self::new(deps…)` instead — the resolved `#[inject]` fields are passed
/// in declaration order. This codegen never sees the impl, so a missing or mis-typed `new`
/// surfaces as an ordinary compile error at the generated call.
///
/// Construction logic (`#[new]`) and lifecycle hooks (`#[on_module_init]`, …) live on the struct's `impl`
/// and reach this path via the `toni::__construct` / `toni::__lifecycle` bridges — the provider
/// wrapper dispatches to them without this codegen needing to see the methods.
pub fn generate_provider_from_struct(
    struct_def: &ItemStruct,
    scope: ProviderScope,
    init_method: Option<String>,
) -> Result<TokenStream> {
    generate_provider_from_struct_with_traits(
        struct_def,
        scope,
        init_method,
        EnhancerTraits::default(),
    )
}

/// Same struct-only DI wiring as [`generate_provider_from_struct`], but with the provider role
/// preset by the caller. The structural macros (`#[websocket_gateway]` / `#[rpc_controller]`) own
/// their role and emit the matching trait impl themselves, so they pass `is_gateway` / `is_rpc_*`
/// here while still field-injecting and dispatching construction/lifecycle through the bridges.
pub fn generate_provider_from_struct_with_traits(
    struct_def: &ItemStruct,
    scope: ProviderScope,
    init_method: Option<String>,
    enhancer_traits: EnhancerTraits,
) -> Result<TokenStream> {
    let struct_name = struct_def.ident.clone();
    let mut dependencies = extract_struct_dependencies(struct_def)?;
    if let Some(init) = init_method {
        dependencies.init_method = Some(init);
    }

    // A derive sees only the struct: no lifecycle hooks (the bridge carries them). The role is
    // supplied by the caller rather than detected from an impl head.
    let lifecycle_hooks = LifecycleHooks::default();

    // Derive can't see the impl, so it dispatches lifecycle through the `#[on_*]` bridge.
    let provider_wrapper = generate_provider_wrapper(
        &struct_name,
        &dependencies,
        scope,
        &enhancer_traits,
        &lifecycle_hooks,
        true,
    );
    let factory = generate_factory(&struct_name, &dependencies, scope, &enhancer_traits);
    let factory_accessor = generate_provider_factory_accessor(&struct_name);

    Ok(quote! {
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
/// #[injectable]
/// pub struct Foo { field: String }
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
pub fn add_clone_and_inject_fields(struct_attrs: &ItemStruct) -> ItemStruct {
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
        // Clone is needed for the provider wrapper; InjectFields keeps the
        // #[inject]/#[default] field attributes valid on the re-emitted struct.
        let derives: syn::Attribute = syn::parse_quote! {
            #[derive(Clone, ::toni::InjectFields)]
        };
        struct_def.attrs.push(derives);
    } else {
        let injectable_derive: syn::Attribute = syn::parse_quote! {
            #[derive(::toni::InjectFields)]
        };
        struct_def.attrs.push(injectable_derive);
    }

    struct_def
}

/// Recursively check if a derive meta contains Clone. Matched by the path's last segment,
/// so `std::clone::Clone` and re-exported Clone derives count too. An aliased derive
/// (`use Clone as C`) or a manual `impl Clone` elsewhere stays invisible — token-level
/// scanning cannot resolve names — and surfaces as a conflicting-implementations error.
fn meta_contains_clone(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.segments.last().is_some_and(|seg| seg.ident == "Clone"),
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

/// The provider struct for `struct_name` at the given scope.
pub(crate) fn provider_ident(struct_name: &Ident) -> Ident {
    Ident::new(&format!("{}Provider", struct_name), struct_name.span())
}

/// The second provider struct an RPC controller carries, holding the dependencies a per-call build
/// resolves from. Which of the two the factory uses is settled at startup.
pub(crate) fn request_provider_ident(struct_name: &Ident) -> Ident {
    Ident::new(
        &format!("{}RequestProvider", struct_name),
        struct_name.span(),
    )
}

fn generate_provider_wrapper(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
    enhancer_traits: &EnhancerTraits,
    lifecycle_hooks: &LifecycleHooks,
    lifecycle_via_bridge: bool,
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => generate_singleton_provider(
            struct_name,
            &provider_ident(struct_name),
            lifecycle_hooks,
            lifecycle_via_bridge,
        ),
        ProviderScope::Request => generate_request_provider(
            struct_name,
            &provider_ident(struct_name),
            dependencies,
            lifecycle_hooks,
        ),
        ProviderScope::Transient => {
            generate_transient_provider(struct_name, dependencies, enhancer_traits, lifecycle_hooks)
        }
    }
}

/// Generate role-push statements to embed inside `build()`, before the concrete
/// `instance: Arc<StructName>` is boxed. Returns a `TokenStream` that pushes
/// each role the struct implements onto a `__roles: Vec<ProviderRole>` local.
///
/// Enhancer roles (guard / interceptor / error-handler / middleware) are detected from the
/// type itself via `toni::__detect` probes — the `impl Guard<HttpContext> for T` is the declaration,
/// no marker required. The gateway role stays flag-driven: it comes from the structural macro that
/// also generates the trait impl and routing. The rpc-controller and grpc-service roles are pushed
/// by their own factories, which decide between the two sources a dispatch target can have.
fn generate_role_pushes(traits: &EnhancerTraits) -> TokenStream {
    let mut pushes = vec![crate::shared::enhancer_emit::value_probe_detection()];

    if traits.is_gateway {
        pushes.push(quote! {
            __roles.push(::toni::traits_helpers::ProviderRole::Gateway(
                ::std::sync::Arc::new(
                    Box::new((*instance).clone()) as Box<dyn ::toni::websocket::GatewayTrait>
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

/// Generate the five `Provider` lifecycle overrides for the derive path, each forwarding to the
/// `#[on_*]` bridge method on the instance. The inherent bridge method (emitted by a hook macro)
/// runs the user hook when present, else the blanket `LifecycleBridge` no-op — so the derive
/// dispatches uniformly without knowing which hooks exist.
///
/// Calls go through UFCS on the concrete type (`Struct::__toni_lc_*(&*self.instance)`) rather than
/// method syntax on `self.instance`. The blanket `impl<T: ?Sized> LifecycleBridge for T` also covers
/// `Arc<Struct>`, so `self.instance.__toni_lc_*()` binds the no-op at the `Arc` level and never
/// derefs to the inherent forwarder on `Struct`. UFCS pins resolution to `Struct`, where the
/// inherent method wins over the blanket when present.
fn generate_bridge_lifecycle_methods(struct_name: &Ident) -> TokenStream {
    quote! {
        async fn on_module_init(&self) -> ::toni::InitResult {
            use ::toni::__lifecycle::LifecycleBridge as _;
            #struct_name::__toni_lc_on_init(&*self.instance).await
        }
        async fn on_application_bootstrap(&self) -> ::toni::InitResult {
            use ::toni::__lifecycle::LifecycleBridge as _;
            #struct_name::__toni_lc_on_bootstrap(&*self.instance).await
        }
        async fn on_module_destroy(&self) {
            use ::toni::__lifecycle::LifecycleBridge as _;
            #struct_name::__toni_lc_on_destroy(&*self.instance).await;
        }
        async fn before_application_shutdown(&self, signal: Option<String>) {
            use ::toni::__lifecycle::LifecycleBridge as _;
            #struct_name::__toni_lc_before_shutdown(&*self.instance, signal).await;
        }
        async fn on_application_shutdown(&self, signal: Option<String>) {
            use ::toni::__lifecycle::LifecycleBridge as _;
            #struct_name::__toni_lc_on_shutdown(&*self.instance, signal).await;
        }
    }
}

fn generate_singleton_provider(
    struct_name: &Ident,
    provider_name: &Ident,
    lifecycle_hooks: &LifecycleHooks,
    lifecycle_via_bridge: bool,
) -> TokenStream {
    let lifecycle_methods = if lifecycle_via_bridge {
        generate_bridge_lifecycle_methods(struct_name)
    } else {
        generate_lifecycle_direct_methods(lifecycle_hooks)
    };

    quote! {
        struct #provider_name {
            instance: ::std::sync::Arc<#struct_name>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Provider for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _ctx: ::toni::ProviderContext,
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
    provider_name: &Ident,
    dependencies: &DependencyInfo,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
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
                    quote! { #field_name: {
                        #[allow(unused_imports)]
                        use ::toni::__construct::OwnedFieldDefaultFallback as _;
                        (&::toni::__construct::OwnedFieldDefault::<#field_type>::new())
                            .field_default(stringify!(#field_name), stringify!(#field_type))
                    } }
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

    // Request-scoped providers require an active execution. Constructing one outside
    // of any execution would silently violate the declared scope contract. Which
    // execution it is does not matter — every transport has one, and the cache that
    // makes the scope mean anything lives on it.
    //
    // Build via the `#[new]` constructor when one exists (inherent fn shadows the blanket
    // `CtorBridge` default), else by field injection — same dispatch as the singleton factory.
    let execute_body = quote! {
        use ::toni::__construct::CtorBridge as _;
        let __exec_ctx = _ctx;
        if __exec_ctx.cache().is_none() {
            panic!(
                "Request-scoped provider '{}' requires an active execution; it cannot be \
                 resolved outside one.",
                ::std::any::type_name::<#struct_name>()
            );
        }
        if let Some(__cached) = __exec_ctx
            .cache()
            .and_then(|__c| __c.get::<#struct_name>())
        {
            return Box::new(__cached);
        }
        // Thread the execution on, so a request-scoped constructor parameter resolves in
        // the same one and is shared rather than rebuilt.
        let instance = match <#struct_name>::__toni_ctor_build(
            &self.dependencies,
            __exec_ctx.clone(),
        ) {
            ::std::option::Option::Some(__fut) => __fut.await,
            ::std::option::Option::None => {
                #(#field_resolutions)*
                #struct_instantiation
            }
        };
        __exec_ctx
            .cache()
            .expect("checked above")
            .insert(instance.clone());
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
                _ctx: ::toni::ProviderContext,
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
                    quote! { #field_name: {
                        #[allow(unused_imports)]
                        use ::toni::__construct::OwnedFieldDefaultFallback as _;
                        (&::toni::__construct::OwnedFieldDefault::<#field_type>::new())
                            .field_default(stringify!(#field_name), stringify!(#field_type))
                    } }
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
                _ctx: ::toni::ProviderContext,
            ) -> Box<dyn ::std::any::Any + Send> {
                // Build via the `#[new]` constructor when one exists, else by field injection.
                // A transient is rebuilt at every injection point, so it is built inside
                // whatever execution asked for it — and its request-scoped fields resolve in
                // that same one rather than starting a new one.
                use ::toni::__construct::CtorBridge as _;
                let __exec_ctx = _ctx;
                let instance = match <#struct_name>::__toni_ctor_build(&self.dependencies, __exec_ctx.clone()) {
                    ::std::option::Option::Some(__fut) => __fut.await,
                    ::std::option::Option::None => {
                        #(#field_resolutions)*
                        #struct_instantiation
                    }
                };
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
                let any_box = provider.execute(vec![], __exec_ctx.clone()).await;
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

                    let any_box = provider.execute(vec![], __exec_ctx.clone()).await;

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
                            let any_box = provider.execute(vec![], __exec_ctx.clone()).await;
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
                        let any_box = provider.execute(vec![], __exec_ctx.clone()).await;
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

/// Assemble the instance: through `init = "…"` when one is named, else as a struct literal with the
/// resolved `#[inject]` fields and the `#[default(…)]` (or `Default`) owned ones.
fn struct_instantiation(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    field_names: &[Ident],
) -> TokenStream {
    if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        return quote! { #struct_name::#init_ident(#(#field_names),*) };
    }

    let owned_field_inits: Vec<_> = dependencies
        .owned_fields
        .iter()
        .map(|(field_name, field_type, default_expr)| {
            if let Some(expr) = default_expr {
                quote! { #field_name: #expr }
            } else {
                quote! { #field_name: {
                    #[allow(unused_imports)]
                    use ::toni::__construct::OwnedFieldDefaultFallback as _;
                    (&::toni::__construct::OwnedFieldDefault::<#field_type>::new())
                        .field_default(stringify!(#field_name), stringify!(#field_type))
                } }
            }
        })
        .collect();

    quote! {
        #struct_name {
            #(#field_names,)*
            #(#owned_field_inits),*
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

    let struct_instantiation = struct_instantiation(struct_name, dependencies, &field_names);

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
                // A `#[new]` constructor supplies its own dependency tokens (inherent fn shadows the
                // blanket `CtorBridge` default); otherwise fall back to the field-injection tokens.
                use ::toni::__construct::CtorBridge as _;
                <#struct_name>::__toni_ctor_tokens().unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            async fn build(
                &self,
                __deps: ::toni::FxHashMap<String, ::toni::traits_helpers::Injectable>,
            ) -> ::toni::traits_helpers::Injectable {
                use ::toni::__construct::CtorBridge as _;
                let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
                    __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();

                #scope_validation

                // Build via the `#[new]` constructor if one exists, else by field injection.
                // Singletons are built at startup, outside any execution.
                let __exec_ctx = ::toni::ProviderContext::None;
                let instance = match <#struct_name>::__toni_ctor_build(&dependencies, __exec_ctx.clone()) {
                    ::std::option::Option::Some(__fut) => ::std::sync::Arc::new(__fut.await),
                    ::std::option::Option::None => ::std::sync::Arc::new({
                        #(#field_resolutions)*
                        #struct_instantiation
                    }),
                };

                let mut __roles = ::std::vec::Vec::new();
                #role_pushes

                let provider = ::std::sync::Arc::new(Box::new(#provider_name { instance }) as Box<dyn ::toni::traits_helpers::Provider>);
                ::toni::traits_helpers::Injectable::new(provider, __roles)
            }
        }
    }
}

/// The factory for an RPC controller — the one provider whose scope is settled at startup rather
/// than at expansion. A controller that declares no scope but depends on a request-scoped provider
/// is elevated, as an HTTP controller is; a declared `scope = "request"` forces the same outcome.
/// The two outcomes differ in what the source is handed: the instance built here, or the per-call
/// provider to build one from.
/// The whole of an RPC controller's wiring: the per-call provider its source resolves from, the
/// `Controller` object the module holds, the `ControllerFactory` that settles the scope at startup,
/// and the accessor `controllers: [Foo]` expands to.
///
/// An RPC controller reaches the framework through the controller path, not the provider path — it is
/// a dispatch target, and nothing may hold one. Only the per-call shape needs a `Provider`, to resolve
/// its dependencies inside the call; the singleton shape is built here and held directly.
pub fn generate_rpc_controller_system(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    request_scoped: bool,
) -> TokenStream {
    let per_call_provider = request_provider_ident(struct_name);
    let source_name = crate::rpc_macro::rpc_controller_attr::rpc_source_ident(struct_name);
    let object_name = Ident::new(
        &format!("{}RpcControllerObject", struct_name),
        struct_name.span(),
    );
    let factory_name = Ident::new(
        &format!("{}ControllerFactory", struct_name),
        struct_name.span(),
    );
    let struct_token = struct_name.to_string();

    let provider = generate_request_provider(
        struct_name,
        &per_call_provider,
        dependencies,
        &LifecycleHooks::default(),
    );

    let (field_resolutions, field_names) = generate_factory_field_resolutions(dependencies);
    let instantiation = struct_instantiation(struct_name, dependencies, &field_names);

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

    // Hooks fire on the instance every call shares. A controller built per call has no such
    // instance; its source fires them on each one it builds.
    let on_singleton = |call: TokenStream| {
        quote! {
            if let ::std::option::Option::Some(__inst) = &self.singleton {
                use ::toni::__lifecycle::LifecycleBridge as _;
                #call
            }
        }
    };
    let init_body = on_singleton(quote! { return #struct_name::__toni_lc_on_init(__inst).await; });
    let boot_body =
        on_singleton(quote! { return #struct_name::__toni_lc_on_bootstrap(__inst).await; });
    let destroy_body = on_singleton(quote! { #struct_name::__toni_lc_on_destroy(__inst).await; });
    let before_body =
        on_singleton(quote! { #struct_name::__toni_lc_before_shutdown(__inst, signal).await; });
    let shutdown_body =
        on_singleton(quote! { #struct_name::__toni_lc_on_shutdown(__inst, signal).await; });

    quote! {
        #provider

        pub struct #object_name {
            source: ::std::sync::Arc<dyn ::toni::rpc::RpcControllerSource>,
            singleton: ::std::option::Option<::std::sync::Arc<#struct_name>>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Controller for #object_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn dispatch(&self) -> ::toni::traits_helpers::Dispatch {
                ::toni::traits_helpers::Dispatch::Rpc(self.source.clone())
            }

            async fn on_module_init(&self) -> ::toni::InitResult {
                #init_body
                Ok(())
            }
            async fn on_application_bootstrap(&self) -> ::toni::InitResult {
                #boot_body
                Ok(())
            }
            async fn on_module_destroy(&self) {
                #destroy_body
            }
            async fn before_application_shutdown(&self, signal: Option<String>) {
                #before_body
            }
            async fn on_application_shutdown(&self, signal: Option<String>) {
                #shutdown_body
            }
        }

        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ControllerFactory for #factory_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                use ::toni::__construct::CtorBridge as _;
                <#struct_name>::__toni_ctor_tokens().unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            async fn build(
                &self,
                dependencies: ::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>,
                >,
            ) -> ::std::sync::Arc<dyn ::toni::traits_helpers::Controller> {
                use ::toni::__construct::CtorBridge as _;

                let mut __request_deps: ::std::vec::Vec<String> = ::std::vec::Vec::new();
                for __token in <Self as ::toni::traits_helpers::ControllerFactory>::get_dependencies(self) {
                    if let Some(__provider) = dependencies.get(&__token) {
                        if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                            __request_deps.push(__token);
                        }
                    }
                }

                if !#request_scoped && !__request_deps.is_empty() {
                    ::toni::tracing::warn!(
                        rpc_controller = #struct_token,
                        request_scoped_deps = ?__request_deps,
                        "RPC controller automatically elevated to request scope due to \
                         request-scoped providers. Silence this by declaring \
                         #[rpc_controller(scope = \"request\")]."
                    );
                }

                if #request_scoped || !__request_deps.is_empty() {
                    let __provider: ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>> =
                        ::std::sync::Arc::new(Box::new(#per_call_provider { dependencies })
                            as Box<dyn ::toni::traits_helpers::Provider>);
                    return ::std::sync::Arc::new(#object_name {
                        source: ::std::sync::Arc::new(#source_name::PerCall(__provider))
                            as ::std::sync::Arc<dyn ::toni::rpc::RpcControllerSource>,
                        singleton: ::std::option::Option::None,
                    });
                }

                // Built at startup, outside any execution, and shared by every call.
                let instance = match <#struct_name>::__toni_ctor_build(
                    &dependencies,
                    ::toni::ProviderContext::None,
                ) {
                    ::std::option::Option::Some(__fut) => ::std::sync::Arc::new(__fut.await),
                    ::std::option::Option::None => ::std::sync::Arc::new({
                        #(#field_resolutions)*
                        #instantiation
                    }),
                };

                ::std::sync::Arc::new(#object_name {
                    source: ::std::sync::Arc::new(#source_name::Singleton(
                        ::std::sync::Arc::new(
                            Box::new((*instance).clone()) as Box<dyn ::toni::rpc::RpcControllerTrait>
                        )
                    )) as ::std::sync::Arc<dyn ::toni::rpc::RpcControllerSource>,
                    singleton: ::std::option::Option::Some(instance),
                })
            }
        }

        impl #struct_name {
            #[doc(hidden)]
            pub fn __toni_controller_factory() -> impl ::toni::traits_helpers::ControllerFactory {
                #factory_name
            }
        }
    }
}

/// The whole of a gRPC service's wiring: the per-call provider its source resolves from, the
/// `Controller` object the module holds, the `ControllerFactory` that settles the scope at startup,
/// and the accessor `controllers: [Foo]` expands to.
///
/// A gRPC service reaches the framework through the controller path, not the provider path — it is a
/// dispatch target, and nothing may hold one. Only the per-call shape needs a `Provider`, to resolve
/// its dependencies inside the call; the singleton shape is built here and held on the source.
fn generate_grpc_service_system(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    request_scoped: bool,
) -> TokenStream {
    let per_call_provider = request_provider_ident(struct_name);
    let source_name = crate::grpc_macro::grpc_methods::grpc_source_ident(struct_name);
    let object_name = Ident::new(
        &format!("{}GrpcServiceObject", struct_name),
        struct_name.span(),
    );
    let factory_name = Ident::new(
        &format!("{}ControllerFactory", struct_name),
        struct_name.span(),
    );
    let struct_token = struct_name.to_string();

    let provider = generate_request_provider(
        struct_name,
        &per_call_provider,
        dependencies,
        &LifecycleHooks::default(),
    );

    let (field_resolutions, field_names) = generate_factory_field_resolutions(dependencies);
    let instantiation = struct_instantiation(struct_name, dependencies, &field_names);

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

    // Hooks fire on the instance every call shares. A service built per call has no such instance;
    // its source fires them on each one it builds.
    let on_singleton = |call: TokenStream| {
        quote! {
            if let #source_name::Singleton(__inst) = &*self.source {
                use ::toni::__lifecycle::LifecycleBridge as _;
                #call
            }
        }
    };
    let init_body = on_singleton(quote! { return #struct_name::__toni_lc_on_init(__inst).await; });
    let boot_body =
        on_singleton(quote! { return #struct_name::__toni_lc_on_bootstrap(__inst).await; });
    let destroy_body = on_singleton(quote! { #struct_name::__toni_lc_on_destroy(__inst).await; });
    let before_body =
        on_singleton(quote! { #struct_name::__toni_lc_before_shutdown(__inst, signal).await; });
    let shutdown_body =
        on_singleton(quote! { #struct_name::__toni_lc_on_shutdown(__inst, signal).await; });

    quote! {
        #provider

        pub struct #object_name {
            source: ::std::sync::Arc<#source_name>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Controller for #object_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn dispatch(&self) -> ::toni::traits_helpers::Dispatch {
                ::toni::traits_helpers::Dispatch::Grpc(
                    self.source.clone() as ::std::sync::Arc<dyn ::toni::adapter::GrpcServiceSource>
                )
            }

            async fn on_module_init(&self) -> ::toni::InitResult {
                #init_body
                Ok(())
            }
            async fn on_application_bootstrap(&self) -> ::toni::InitResult {
                #boot_body
                Ok(())
            }
            async fn on_module_destroy(&self) {
                #destroy_body
            }
            async fn before_application_shutdown(&self, signal: Option<String>) {
                #before_body
            }
            async fn on_application_shutdown(&self, signal: Option<String>) {
                #shutdown_body
            }
        }

        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ControllerFactory for #factory_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                use ::toni::__construct::CtorBridge as _;
                <#struct_name>::__toni_ctor_tokens().unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            async fn build(
                &self,
                dependencies: ::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>,
                >,
            ) -> ::std::sync::Arc<dyn ::toni::traits_helpers::Controller> {
                use ::toni::__construct::CtorBridge as _;

                let mut __request_deps: ::std::vec::Vec<String> = ::std::vec::Vec::new();
                for __token in <Self as ::toni::traits_helpers::ControllerFactory>::get_dependencies(self) {
                    if let Some(__provider) = dependencies.get(&__token) {
                        if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                            __request_deps.push(__token);
                        }
                    }
                }

                if !#request_scoped && !__request_deps.is_empty() {
                    ::toni::tracing::warn!(
                        grpc_service = #struct_token,
                        request_scoped_deps = ?__request_deps,
                        "gRPC service automatically elevated to request scope due to \
                         request-scoped providers. Silence this by declaring \
                         #[grpc_service(scope = \"request\", ...)]."
                    );
                }

                let __source = if #request_scoped || !__request_deps.is_empty() {
                    let __provider: ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>> =
                        ::std::sync::Arc::new(Box::new(#per_call_provider { dependencies })
                            as Box<dyn ::toni::traits_helpers::Provider>);
                    #source_name::PerCall(__provider)
                } else {
                    // Built at startup, outside any execution, and shared by every call.
                    let instance = match <#struct_name>::__toni_ctor_build(
                        &dependencies,
                        ::toni::ProviderContext::None,
                    ) {
                        ::std::option::Option::Some(__fut) => ::std::sync::Arc::new(__fut.await),
                        ::std::option::Option::None => ::std::sync::Arc::new({
                            #(#field_resolutions)*
                            #instantiation
                        }),
                    };
                    #source_name::Singleton(instance)
                };

                ::std::sync::Arc::new(#object_name {
                    source: ::std::sync::Arc::new(__source),
                })
            }
        }

        impl #struct_name {
            #[doc(hidden)]
            pub fn __toni_controller_factory() -> impl ::toni::traits_helpers::ControllerFactory {
                #factory_name
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

    let enhancer_preamble = if factory_role_pushes.is_empty() {
        quote! {}
    } else {
        quote! {
            let __has_request_deps = __deps.values().any(|inj|
                matches!(inj.instance.get_scope(), ::toni::ProviderScope::Request)
            );
            let __all_deps = ::std::sync::Arc::new(
                __deps.iter()
                    .map(|(k, inj)| (k.clone(), inj.instance.clone()))
                    .collect::<::toni::FxHashMap<_, _>>()
            );
        }
    };

    let build_body = quote! {
        #enhancer_preamble
        let dependencies: ::toni::FxHashMap<String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>> =
            __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
        let __provider: ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>> =
            ::std::sync::Arc::new(Box::new(#provider_name { dependencies }) as Box<dyn ::toni::traits_helpers::Provider>);
        let mut __roles = ::std::vec::Vec::new();
        #factory_role_pushes
        ::toni::traits_helpers::Injectable::new(__provider, __roles)
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
                // A `#[new]` constructor supplies its own dependency tokens; else fall back to the
                // field-injection tokens.
                use ::toni::__construct::CtorBridge as _;
                <#struct_name>::__toni_ctor_tokens().unwrap_or_else(|| vec![#(#dependency_tokens),*])
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
                // A `#[new]` constructor supplies its own dependency tokens; else fall back to the
                // field-injection tokens.
                use ::toni::__construct::CtorBridge as _;
                <#struct_name>::__toni_ctor_tokens().unwrap_or_else(|| vec![#(#dependency_tokens),*])
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
                    __exec_ctx.clone()
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
                    __exec_ctx.clone()
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

/// Generates the per-request enhancer-factory structs (`Dyn*Factory` implementors) for a
/// request/transient-scoped provider, and the role pushes that register them.
///
/// Roles are detected from the type, not a marker: a `Dyn*Factory` is emitted for every enhancer
/// kind (the `toni::__detect` value-probe inside `create()` compiles for any `T` — it yields the
/// coerced trait object for an implementor and never runs otherwise), and each role push is gated
/// by a `toni::__detect` type-level probe over the concrete struct so only the kinds the type
/// actually implements register. `enhancer_traits` no longer drives this — only structural roles
/// (gateway/rpc-controller/grpc-service) remain flag-driven elsewhere.
///
/// Returns `(struct_defs, role_pushes)`:
/// - `struct_defs`: emitted before the provider factory struct
/// - `role_pushes`: emitted inside `build()`, assumes `__all_deps` and `__has_request_deps` are in scope
fn generate_dyn_factories(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    _enhancer_traits: &EnhancerTraits,
) -> (TokenStream, TokenStream) {
    use crate::shared::enhancer_emit::EnhancerKind;

    let active_kinds: Vec<EnhancerKind> = EnhancerKind::all().into_iter().collect();

    let (field_resolutions, field_names) = generate_create_field_resolutions(dependencies);

    // Struct construction — same shape as request/transient provider execute()
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        let is_from_request = init_fn == "from_request";
        if is_from_request {
            if field_names.is_empty() {
                quote! { #struct_name::#init_ident(__exec_ctx.request_parts().expect("HTTP request context required")) }
            } else {
                quote! { #struct_name::#init_ident(__exec_ctx.request_parts().expect("HTTP request context required"), #(#field_names),*) }
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
                    quote! { #field_name: {
                        #[allow(unused_imports)]
                        use ::toni::__construct::OwnedFieldDefaultFallback as _;
                        (&::toni::__construct::OwnedFieldDefault::<#field_type>::new())
                            .field_default(stringify!(#field_name), stringify!(#field_type))
                    } }
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

    // One shared builder per provider: it owns the (heavy) dependency-resolution + construction
    // logic once. Each `Dyn*Factory` impl is a thin shim that builds via this and value-probes the
    // result to its role trait object — so the field-resolution code isn't duplicated 11 times.
    let builder_struct_name = Ident::new(
        &format!("__Toni{}EnhancerBuilder", struct_name),
        struct_name.span(),
    );

    let builder_def = quote! {
        struct #builder_struct_name {
            all_deps: #deps_arc_ty,
            has_request_deps: bool,
        }

        impl #builder_struct_name {
            async fn __build_instance<'a>(
                &'a self,
                __exec_ctx: ::toni::ProviderContext,
            ) -> #struct_name {
                // A `#[new]` constructor takes over construction; otherwise fall back to field
                // injection. Both thread the execution so request-scoped sub-dependencies
                // resolve in it rather than in one of their own.
                use ::toni::__construct::CtorBridge as _;
                if let ::std::option::Option::Some(__fut) =
                    <#struct_name>::__toni_ctor_build(&*self.all_deps, __exec_ctx.clone())
                {
                    return __fut.await;
                }
                let all_deps = self.all_deps.clone();
                #(#field_resolutions)*
                #struct_instantiation
            }
        }
    };

    let mut impl_defs = Vec::new();
    let mut role_push_stmts = Vec::new();

    for kind in active_kinds {
        let spec = kind.spec();
        let trait_path = &spec.trait_path;
        let factory_trait_path = &spec.dyn_factory_trait;
        let role_variant = &spec.role_variant;
        let entry_path = &spec.entry_path;
        let context_path = &spec.context_path;
        let provider_ctx_variant = &spec.provider_ctx_variant;
        let value_probe = Ident::new(&format!("{}Probe", spec.factory_suffix), struct_name.span());
        let type_probe = Ident::new(
            &format!("{}TypeProbe", spec.factory_suffix),
            struct_name.span(),
        );

        // `create()` builds via the shared builder, then value-probes the instance to the role
        // trait object. The probe compiles for any `T` (fallback returns `None`); this impl's role
        // is only ever registered when the type-probe below confirms `T` implements the trait, so
        // the `expect` cannot fire.
        impl_defs.push(quote! {
            impl #factory_trait_path for #builder_struct_name {
                fn create<'a>(
                    &'a self,
                    __ctx: &'a #context_path,
                ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<
                    Output = ::std::sync::Arc<dyn #trait_path + Send + Sync>
                > + Send + 'a>> {
                    ::std::boxed::Box::pin(async move {
                        use ::toni::__detect::prelude::*;
                        let instance = self.__build_instance(#provider_ctx_variant(__ctx.clone())).await;
                        ::toni::__detect::#value_probe(::std::sync::Arc::new(instance))
                            .detect()
                            .expect("enhancer factory registered only when the type implements the role")
                            as ::std::sync::Arc<dyn #trait_path + Send + Sync>
                    })
                }
            }
        });
        // Gate registration on the type-level probe over the concrete struct: only kinds the type
        // actually implements get a factory pushed. `.is()` resolves to the inherent method (true)
        // when `#struct_name` implements the trait, else the in-scope fallback (false).
        role_push_stmts.push(quote! {
            if ::toni::__detect::#type_probe::<#struct_name>(::std::marker::PhantomData).is() {
                __roles.push(#role_variant(
                    #entry_path::Factory(
                        ::std::sync::Arc::new(#builder_struct_name {
                            all_deps: __all_deps.clone(),
                            has_request_deps: __has_request_deps,
                        })
                    )
                ));
            }
        });
    }

    let struct_defs = quote! {
        #builder_def
        #(#impl_defs)*
    };
    let role_pushes = quote! {
        {
            use ::toni::__detect::prelude::*;
            #(#role_push_stmts)*
        }
    };

    (struct_defs, role_pushes)
}
