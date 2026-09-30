//! Singleton Instance Injection Implementation
//!
//! Architecture:
//! 1. User struct with REAL fields (unchanged)
//! 2. `AppServiceProvider` (implements `Provider`) — holds `Arc<AppService>`, created once at startup
//! 3. `AppServiceProviderFactory` (implements `ProviderFactory`) — zero-sized descriptor; resolves
//!    deps and calls `build()` once to produce the `Provider` instance

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemStruct, Result, Type};

use crate::{
    shared::{dependency_info::DependencyInfo, scope_parser::ProviderScope},
    utils::extracts::{extract_arc_inner, extract_struct_dependencies, extract_vec_arc_dyn_inner},
};

/// Structural roles the surrounding macro assigns to a provider.
///
/// Enhancer roles (guard / interceptor / error-handler / middleware) are NOT here — those
/// are detected from the type's trait impls via `ulo::__detect` at the factory. These three are
/// driven by the structural macros (`#[websocket_gateway]`),
/// which generate the corresponding trait impl and routing, so the macro that emits them already
/// knows the role.
#[derive(Debug, Clone, Default)]
pub struct EnhancerTraits {
    pub is_gateway: bool,
}

/// Construction logic (`#[new]`) and lifecycle hooks (`#[on_module_init]`, …) live on the struct's `impl`
/// and reach this path via the `ulo::__construct` / `ulo::__lifecycle` bridges — the provider
/// wrapper dispatches to them without this codegen needing to see the methods.
pub fn generate_provider_from_struct(
    struct_def: &ItemStruct,
    scope: ProviderScope,
) -> Result<TokenStream> {
    generate_provider_from_struct_with_traits(struct_def, scope, EnhancerTraits::default())
}

/// Same struct-only DI wiring as [`generate_provider_from_struct`], but with the provider role
/// preset by the caller. The structural macros (`#[websocket_gateway]`) own
/// their role and emit the matching trait impl themselves, so they pass `is_gateway` / `is_rpc_*`
/// here while still field-injecting and dispatching construction/lifecycle through the bridges.
pub fn generate_provider_from_struct_with_traits(
    struct_def: &ItemStruct,
    scope: ProviderScope,
    enhancer_traits: EnhancerTraits,
) -> Result<TokenStream> {
    let struct_name = struct_def.ident.clone();
    let dependencies = extract_struct_dependencies(struct_def)?;

    // The struct macro can't see the impl, so it dispatches lifecycle through the `#[on_*]`
    // bridge. The role is supplied by the caller rather than detected from an impl head.
    let provider_wrapper =
        generate_provider_wrapper(&struct_name, &dependencies, scope, &enhancer_traits);
    let factory = generate_factory(&struct_name, &dependencies, scope, &enhancer_traits);
    let factory_accessor = generate_provider_factory_accessor(&struct_name);
    let default_checks = crate::shared::default_beside_new::default_beside_new(struct_def);
    let shared_flag = shared_flag(&struct_name, scope);
    let plain_reads = plain_read_checks(&dependencies);

    Ok(quote! {
        #provider_wrapper
        #factory
        #factory_accessor
        #default_checks
        #shared_flag
        #plain_reads
    })
}

/// The inherent const a singleton or execution-scoped type carries, over the blanket
/// `ulo::__di::SharedFlag` default: its provider hands out one shared `Arc`, which a plain field
/// or parameter of the type cannot hold.
fn shared_flag(struct_name: &Ident, scope: ProviderScope) -> TokenStream {
    match scope {
        ProviderScope::Singleton | ProviderScope::Execution => quote! {
            impl #struct_name {
                #[doc(hidden)]
                pub const __ULO_SHARED: bool = true;
            }
        },
        ProviderScope::Transient => TokenStream::new(),
    }
}

/// One `const` item per `#[inject]` field written as a plain type, failing const evaluation when
/// that type is handed out as one shared instance. Spanned at the field's type.
pub(crate) fn plain_read_checks(dependencies: &DependencyInfo) -> TokenStream {
    dependencies
        .fields
        .iter()
        .map(|(_, ty, _)| plain_read_check(ty))
        .collect()
}

/// The check `plain_read_checks` emits for one field or parameter of type `ty`: nothing for
/// `Arc<T>` or `Vec<Arc<dyn Trait>>`, which hold any binding, and for a plain type a `const` item
/// reading `ulo::__di::SharedFlag`, whose inherent shadow a shared `#[injectable]` type carries.
pub(crate) fn plain_read_check(ty: &Type) -> TokenStream {
    if extract_arc_inner(ty).is_some() || extract_vec_arc_dyn_inner(ty).is_some() {
        return TokenStream::new();
    }
    let written = crate::shared::type_display::type_display(ty);
    let message =
        format!("`{written}` is handed out as one shared instance: write `Arc<{written}>`");
    quote::quote_spanned! {syn::spanned::Spanned::span(ty)=>
        const _: () = {
            #[allow(unused_imports)]
            use ::ulo::__di::SharedFlag as _;
            if <#ty>::__ULO_SHARED {
                ::core::panic!(#message);
            }
        };
    }
}

/// Re-emit the struct deriving `InjectFields`, which keeps `#[inject]` and `#[default]` valid as
/// inert field attributes. The container holds the one instance behind an `Arc`, so nothing
/// clones it and no `Clone` is derived.
pub fn add_inject_fields(struct_attrs: &ItemStruct) -> ItemStruct {
    let mut struct_def = struct_attrs.clone();
    let injectable_derive: syn::Attribute = syn::parse_quote! {
        #[derive(::ulo::InjectFields)]
    };
    struct_def.attrs.push(injectable_derive);
    struct_def
}

fn generate_provider_factory_accessor(struct_name: &Ident) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ProviderFactory", struct_name),
        struct_name.span(),
    );
    quote! {
        impl ::ulo::di::DeclaresProvider for #struct_name {
            fn provider_factory() -> impl ::ulo::spi::ProviderFactory + 'static {
                #factory_name
            }
        }

        impl ::ulo::di::Key for #struct_name {
            type Value = Self;
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
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => {
            generate_singleton_provider(struct_name, &provider_ident(struct_name))
        }
        ProviderScope::Execution => {
            generate_execution_provider(struct_name, &provider_ident(struct_name), dependencies)
        }
        ProviderScope::Transient => {
            generate_transient_provider(struct_name, dependencies, enhancer_traits)
        }
    }
}

/// Generate role-push statements to embed inside `build()`, before the concrete
/// `instance: Arc<StructName>` is boxed. Returns a `TokenStream` that pushes
/// each role the struct implements onto a `__roles: Vec<ProviderRole>` local.
///
/// Enhancer roles (guard / interceptor / error-handler / middleware) are detected from the
/// type itself via `ulo::__detect` probes — the `impl Guard<HttpContext> for T` is the declaration,
/// no marker required. The gateway role stays flag-driven: it comes from the structural macro that
/// also generates the trait impl and routing. The rpc-controller and grpc-service roles are pushed
/// by their own factories, which decide between the two sources a dispatch target can have.
fn generate_role_pushes(traits: &EnhancerTraits) -> TokenStream {
    let mut pushes = vec![crate::shared::enhancer_emit::value_probe_detection()];

    if traits.is_gateway {
        pushes.push(quote! {
            __roles.push(::ulo::spi::ProviderRole::Gateway(
                instance.clone() as ::std::sync::Arc<dyn ::ulo::ws::Gateway>
            ));
        });
    }

    quote! { #(#pushes)* }
}

/// Generate the five `Provider` lifecycle overrides for the derive path, each forwarding to the
/// `#[on_*]` bridge method on the instance. The inherent bridge method (emitted by a hook macro)
/// runs the user hook when present, else the blanket `LifecycleBridge` no-op — so the derive
/// dispatches uniformly without knowing which hooks exist.
///
/// Calls go through UFCS on the concrete type (`Struct::__ulo_lc_*(&*self.instance)`) rather than
/// method syntax on `self.instance`. The blanket `impl<T: ?Sized> LifecycleBridge for T` also covers
/// `Arc<Struct>`, so `self.instance.__ulo_lc_*()` binds the no-op at the `Arc` level and never
/// derefs to the inherent forwarder on `Struct`. UFCS pins resolution to `Struct`, where the
/// inherent method wins over the blanket when present.
fn generate_bridge_lifecycle_methods(struct_name: &Ident) -> TokenStream {
    quote! {
        async fn on_module_init(&self) -> ::ulo::di::InitResult {
            use ::ulo::__lifecycle::LifecycleBridge as _;
            #struct_name::__ulo_lc_on_init(&*self.instance).await
        }
        async fn on_application_bootstrap(&self) -> ::ulo::di::InitResult {
            use ::ulo::__lifecycle::LifecycleBridge as _;
            #struct_name::__ulo_lc_on_bootstrap(&*self.instance).await
        }
        async fn on_module_destroy(&self) {
            use ::ulo::__lifecycle::LifecycleBridge as _;
            #struct_name::__ulo_lc_on_destroy(&*self.instance).await;
        }
        async fn before_application_shutdown(&self, signal: Option<String>) {
            use ::ulo::__lifecycle::LifecycleBridge as _;
            #struct_name::__ulo_lc_before_shutdown(&*self.instance, signal).await;
        }
        async fn on_application_shutdown(&self, signal: Option<String>) {
            use ::ulo::__lifecycle::LifecycleBridge as _;
            #struct_name::__ulo_lc_on_shutdown(&*self.instance, signal).await;
        }
    }
}

fn generate_singleton_provider(struct_name: &Ident, provider_name: &Ident) -> TokenStream {
    let lifecycle_methods = generate_bridge_lifecycle_methods(struct_name);

    quote! {
        struct #provider_name {
            instance: ::std::sync::Arc<#struct_name>,
        }

        #[::ulo::async_trait]
        impl ::ulo::spi::Provider for #provider_name {
            async fn resolve(
                &self,
                _ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<
                Box<dyn ::std::any::Any + Send>,
                ::ulo::di::ResolutionError,
            > {
                ::std::result::Result::Ok(Box::new(self.instance.clone()))
            }

            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn scope(&self) -> ::ulo::di::ProviderScope {
                ::ulo::di::ProviderScope::Singleton
            }

            fn shape(&self) -> ::ulo::spi::Shape {
                ::ulo::spi::Shape::Shared
            }

            #lifecycle_methods
        }
    }
}

fn generate_execution_provider(
    struct_name: &Ident,
    provider_name: &Ident,
    dependencies: &DependencyInfo,
) -> TokenStream {
    let (field_resolutions, field_names) = field_resolutions(
        dependencies,
        quote! { self.dependencies },
        quote! { __exec_ctx.clone() },
    );

    let struct_instantiation = struct_instantiation(struct_name, dependencies, &field_names);

    // The instance lives in the execution's cache, which only an execution has; which
    // transport's does not matter. Outside one the answer is the refusal, which a provider
    // built at startup and injecting this one fails its build with.
    //
    // Build via the `#[new]` constructor when one exists (its inherent const shadows the blanket
    // `CtorBridge` default), else by field injection — same dispatch as the singleton factory.
    let execute_body = quote! {
        use ::ulo::__construct::CtorBridge as _;
        let __exec_ctx = _ctx;
        if __exec_ctx.cache().is_none() {
            return ::std::result::Result::Err(::ulo::di::ResolutionError::ExecutionRequired {
                token: ::ulo::di::token_of::<#struct_name>(),
            });
        }
        if let Some(__cached) = __exec_ctx
            .cache()
            .and_then(|__c| __c.get::<#struct_name>())
        {
            return ::std::result::Result::Ok(Box::new(__cached));
        }
        // Thread the execution on, so an execution-scoped constructor parameter resolves in
        // the same one and is shared rather than rebuilt.
        let instance = match <#struct_name>::__ULO_ONE_NEW_PER_TYPE
            .map(|__ctor| (__ctor.build)(&self.dependencies, __exec_ctx.clone()))
        {
            ::std::option::Option::Some(__fut) => __fut.await?,
            ::std::option::Option::None => {
                #(#field_resolutions)*
                #struct_instantiation
            }
        };
        let instance = __exec_ctx
            .cache()
            .expect("checked above")
            .insert(::std::sync::Arc::new(instance));
        ::std::result::Result::Ok(Box::new(instance))
    };

    quote! {

        struct #provider_name {
            dependencies: ::ulo::FxHashMap<
                String,
                ::std::sync::Arc<dyn ::ulo::spi::Provider>
            >,
        }

        #[::ulo::async_trait]
        impl ::ulo::spi::Provider for #provider_name {
            async fn resolve(
                &self,
                _ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<
                Box<dyn ::std::any::Any + Send>,
                ::ulo::di::ResolutionError,
            > {
                #execute_body
            }

            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn scope(&self) -> ::ulo::di::ProviderScope {
                ::ulo::di::ProviderScope::Execution
            }

            fn shape(&self) -> ::ulo::spi::Shape {
                ::ulo::spi::Shape::Shared
            }
        }
    }
}

/// The construction machine behind every dispatch target, emitted once per `#[controller]`
/// struct: the per-call provider its source resolves from, the `Controller` object the module
/// holds (lifecycle hooks reaching the `Singleton` arm), the `ControllerFactory` whose `build`
/// runs the elevation scan and settles the `DispatchSource` variant, and the accessor
/// `controllers: [Foo]` expands to. The transport is not named here: `dispatch()` resolves
/// through the `DispatchBridge`, which the handler-impl macro shadows.
pub(crate) fn generate_dispatch_system(struct_name: &Ident) -> TokenStream {
    let per_call_provider = request_provider_ident(struct_name);
    let object_name = Ident::new(
        &format!("{}ControllerObject", struct_name),
        struct_name.span(),
    );
    let factory_name = Ident::new(
        &format!("{}ControllerFactory", struct_name),
        struct_name.span(),
    );
    let struct_token = struct_name.to_string();

    let provider = generate_dispatch_provider(struct_name, &per_call_provider);

    // Hooks fire on the instance every call shares. A target built per call has no such
    // instance; its provider fires init/bootstrap on each one it builds.
    let on_singleton = |call: TokenStream| {
        quote! {
            if let ::ulo::__enhancer::DispatchSource::Singleton(__inst) = &self.source {
                use ::ulo::__lifecycle::LifecycleBridge as _;
                #call
            }
        }
    };
    let init_body = on_singleton(quote! { return #struct_name::__ulo_lc_on_init(__inst).await; });
    let boot_body =
        on_singleton(quote! { return #struct_name::__ulo_lc_on_bootstrap(__inst).await; });
    let destroy_body = on_singleton(quote! { #struct_name::__ulo_lc_on_destroy(__inst).await; });
    let before_body =
        on_singleton(quote! { #struct_name::__ulo_lc_before_shutdown(__inst, signal).await; });
    let shutdown_body =
        on_singleton(quote! { #struct_name::__ulo_lc_on_shutdown(__inst, signal).await; });

    quote! {
        #provider

        pub struct #object_name {
            source: ::ulo::__enhancer::DispatchSource<#struct_name>,
        }

        #[::ulo::async_trait]
        impl ::ulo::dispatch::Controller for #object_name {
            fn token(&self) -> String {
                #struct_token.to_string()
            }

            fn targets(&self) -> ::ulo::dispatch::Targets {
                use ::ulo::__dispatch::DispatchBridge as _;
                <#struct_name>::__ulo_dispatch(&self.source)
            }

            async fn on_module_init(&self) -> ::ulo::di::InitResult {
                #init_body
                Ok(())
            }
            async fn on_application_bootstrap(&self) -> ::ulo::di::InitResult {
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

        #[::ulo::async_trait]
        impl ::ulo::dispatch::ControllerFactory for #factory_name {
            fn token(&self) -> String {
                #struct_token.to_string()
            }

            fn dependency_tokens(&self) -> Vec<String> {
                <#struct_name>::__ulo_dependencies()
            }

            fn value_dependencies(&self) -> Vec<(String, &'static str)> {
                <#struct_name>::__ulo_value_dependencies()
            }

            async fn build(
                &self,
                dependencies: ::ulo::FxHashMap<
                    String,
                    ::std::sync::Arc<dyn ::ulo::spi::Provider>,
                >,
            ) -> ::ulo::spi::BuildResult<::std::sync::Arc<dyn ::ulo::dispatch::Controller>> {
                let __force_execution: bool = <#struct_name>::__ulo_is_execution_scoped();
                let __declared =
                    <Self as ::ulo::dispatch::ControllerFactory>::dependency_tokens(self);
                let __execution_deps = ::ulo::__enhancer::execution_scoped_dependencies(
                    &__declared,
                    &dependencies,
                );

                if !__force_execution && !__execution_deps.is_empty() {
                    ::ulo::tracing::warn!(
                        controller = #struct_token,
                        execution_scoped_deps = ?__execution_deps,
                        "Controller automatically elevated to execution scope due to execution-scoped \
                         providers. Silence this by declaring #[controller(scope = \"execution\")]."
                    );
                }

                let __source = if __force_execution || !__execution_deps.is_empty() {
                    ::ulo::__enhancer::DispatchSource::PerCall(
                        ::std::sync::Arc::new(#per_call_provider { dependencies }) as ::std::sync::Arc<dyn ::ulo::spi::Provider>,
                    )
                } else {
                    // Built at startup, outside any execution, and shared by every call.
                    ::ulo::__enhancer::DispatchSource::Singleton(::std::sync::Arc::new(
                        <#struct_name>::__ulo_build_from_deps(
                            &dependencies,
                            ::ulo::di::Execution::None,
                        )
                        .await?,
                    ))
                };

                ::std::result::Result::Ok(::std::sync::Arc::new(#object_name { source: __source }))
            }
        }

        impl ::ulo::di::DeclaresController for #struct_name {
            fn controller_factory() -> impl ::ulo::dispatch::ControllerFactory + 'static {
                #factory_name
            }
        }

        impl ::ulo::di::Key for #struct_name {
            type Value = Self;
        }
    }
}

/// The per-call provider behind a `DispatchSource::PerCall` arm: it answers with
/// `Result<Arc<T>, HookFailed>`, fires init/bootstrap at the build site where hook resolution sees
/// the concrete type, caches the `Arc` in the execution once both return `Ok`, and builds through
/// the struct's `__ulo_build_from_deps` bridge. Nothing clones the target, so the struct needs no
/// `Clone`.
pub(crate) fn generate_dispatch_provider(
    struct_name: &Ident,
    provider_name: &Ident,
) -> TokenStream {
    quote! {
        struct #provider_name {
            dependencies: ::ulo::FxHashMap<
                String,
                ::std::sync::Arc<dyn ::ulo::spi::Provider>
            >,
        }

        #[::ulo::async_trait]
        impl ::ulo::spi::Provider for #provider_name {
            async fn resolve(
                &self,
                _ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<
                Box<dyn ::std::any::Any + Send>,
                ::ulo::di::ResolutionError,
            > {
                let __exec_ctx = _ctx;
                if __exec_ctx.cache().is_none() {
                    return ::std::result::Result::Err(
                        ::ulo::di::ResolutionError::ExecutionRequired {
                            token: ::ulo::di::token_of::<#struct_name>(),
                        },
                    );
                }
                if let Some(__cached) = __exec_ctx
                    .cache()
                    .and_then(|__c| __c.get::<#struct_name>())
                {
                    return ::std::result::Result::Ok(Box::new(::std::result::Result::<
                        ::std::sync::Arc<#struct_name>,
                        ::ulo::errors::HookFailed,
                    >::Ok(__cached)));
                }
                // `__exec_ctx` threads into the build, so an execution-scoped dependency resolves
                // in the same execution and is shared rather than rebuilt.
                let __instance = <#struct_name>::__ulo_build_from_deps(
                    &self.dependencies,
                    __exec_ctx.clone(),
                )
                .await?;
                let __instance = ::std::sync::Arc::new(__instance);
                // Hooks complete before the cache holds the instance, so nothing is handed a
                // pre-init one, and a failed hook leaves nothing cached for the call to reuse.
                let __hooks: ::std::result::Result<(), ::ulo::errors::HookFailed> = async {
                    use ::ulo::__lifecycle::LifecycleBridge as _;
                    #struct_name::__ulo_lc_on_init(&__instance).await.map_err(|__e| {
                        ::ulo::errors::HookFailed::new(
                            ::ulo::di::token_of::<#struct_name>(),
                            "on_module_init",
                            __e,
                        )
                    })?;
                    #struct_name::__ulo_lc_on_bootstrap(&__instance).await.map_err(|__e| {
                        ::ulo::errors::HookFailed::new(
                            ::ulo::di::token_of::<#struct_name>(),
                            "on_application_bootstrap",
                            __e,
                        )
                    })
                }
                .await;
                if let ::std::result::Result::Err(__failed) = __hooks {
                    return ::std::result::Result::Ok(Box::new(::std::result::Result::<
                        ::std::sync::Arc<#struct_name>,
                        ::ulo::errors::HookFailed,
                    >::Err(__failed)));
                }
                let __instance = __exec_ctx
                    .cache()
                    .expect("checked above")
                    .insert(__instance);
                ::std::result::Result::Ok(Box::new(::std::result::Result::<
                    ::std::sync::Arc<#struct_name>,
                    ::ulo::errors::HookFailed,
                >::Ok(__instance)))
            }

            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn scope(&self) -> ::ulo::di::ProviderScope {
                ::ulo::di::ProviderScope::Execution
            }
        }
    }
}

fn generate_transient_provider(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    _enhancer_traits: &EnhancerTraits,
) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());

    let (field_resolutions, field_names) = field_resolutions(
        dependencies,
        quote! { self.dependencies },
        quote! { __exec_ctx.clone() },
    );

    let struct_instantiation = struct_instantiation(struct_name, dependencies, &field_names);

    quote! {

        struct #provider_name {
            dependencies: ::ulo::FxHashMap<
                String,
                ::std::sync::Arc<dyn ::ulo::spi::Provider>
            >,
        }

        #[::ulo::async_trait]
        impl ::ulo::spi::Provider for #provider_name {
            async fn resolve(
                &self,
                _ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<
                Box<dyn ::std::any::Any + Send>,
                ::ulo::di::ResolutionError,
            > {
                // Build via the `#[new]` constructor when one exists, else by field injection.
                // A transient is rebuilt at every injection point, so it is built inside
                // whatever execution asked for it — and its execution-scoped fields resolve in
                // that same one rather than starting a new one.
                use ::ulo::__construct::CtorBridge as _;
                let __exec_ctx = _ctx;
                let instance = match <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.build)(&self.dependencies, __exec_ctx.clone())) {
                    ::std::option::Option::Some(__fut) => __fut.await?,
                    ::std::option::Option::None => {
                        #(#field_resolutions)*
                        #struct_instantiation
                    }
                };
                ::std::result::Result::Ok(Box::new(instance))
            }

            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }


            fn scope(&self) -> ::ulo::di::ProviderScope {
                ::ulo::di::ProviderScope::Transient
            }
        }
    }
}

/// The `#[inject]` fields' resolutions, each read from the dependency map `deps` and resolved in
/// the execution `ctx`. The execution and transient providers pass `self.dependencies` and the
/// execution being served; the singleton factory passes the map its `build` collects from
/// `__deps`, and no execution.
fn field_resolutions(
    dependencies: &DependencyInfo,
    deps: TokenStream,
    ctx: TokenStream,
) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    for (field_name, full_type, lookup_token_expr) in &dependencies.fields {
        let field_name_str = field_name.to_string();
        let take = take_answer(full_type, quote! { provider.resolve(#ctx).await? });
        resolutions.push(quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let provider = #deps
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                #take
            };
        });
        field_names.push(field_name.clone());
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
        ProviderScope::Execution => {
            generate_request_factory(struct_name, dependencies, enhancer_traits)
        }
        ProviderScope::Transient => {
            generate_transient_factory(struct_name, dependencies, enhancer_traits)
        }
    }
}

/// Assemble the instance as a struct literal with the resolved `#[inject]` fields and the
/// `#[default(…)]` (or `Default`) owned ones.
fn struct_instantiation(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    field_names: &[Ident],
) -> TokenStream {
    let owned_field_inits: Vec<_> = dependencies
        .owned_fields
        .iter()
        .map(|(field_name, field_type, default_expr)| {
            if let Some(expr) = default_expr {
                quote! { #field_name: #expr }
            } else {
                quote! { #field_name: {
                    #[allow(unused_imports)]
                    use ::ulo::__construct::OwnedFieldDefaultFallback as _;
                    (&::ulo::__construct::OwnedFieldDefault::<#field_type>::new())
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

    let (field_resolutions, field_names) = field_resolutions(
        dependencies,
        quote! { dependencies },
        quote! { ::ulo::di::Execution::None },
    );

    let struct_instantiation = struct_instantiation(struct_name, dependencies, &field_names);

    let dependency_tokens: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .collect();
    let value_reads = value_reads(dependencies.fields.iter().map(|(_, ty, tok)| (ty, tok)));

    let role_pushes = generate_role_pushes(enhancer_traits);

    quote! {
        pub struct #factory_name;

        #[::ulo::async_trait]
        impl ::ulo::spi::ProviderFactory for #factory_name {
            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn dependency_tokens(&self) -> Vec<String> {
                // A `#[new]` constructor supplies its own dependency tokens (its inherent const
                // shadows the blanket `CtorBridge` default); otherwise fall back to the field-injection
                // tokens.
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.tokens)()).unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            fn value_dependencies(&self) -> Vec<(String, &'static str)> {
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.values)()).unwrap_or_else(|| #value_reads)
            }

            async fn build(
                &self,
                __deps: ::ulo::FxHashMap<String, ::ulo::spi::Registration>,
            ) -> ::ulo::spi::BuildResult<::ulo::spi::Registration> {
                use ::ulo::__construct::CtorBridge as _;
                let dependencies: ::ulo::FxHashMap<String, ::std::sync::Arc<dyn ::ulo::spi::Provider>> =
                    __deps.into_iter().map(|(k, registration)| (k, registration.instance)).collect();

                // Build via the `#[new]` constructor if one exists, else by field injection.
                // Singletons are built at startup, outside any execution, and a dependency that
                // needs one answers the refusal this build fails with.
                let __exec_ctx = ::ulo::di::Execution::None;
                let instance = match <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.build)(&dependencies, __exec_ctx.clone())) {
                    ::std::option::Option::Some(__fut) => ::std::sync::Arc::new(__fut.await?),
                    ::std::option::Option::None => ::std::sync::Arc::new({
                        #(#field_resolutions)*
                        #struct_instantiation
                    }),
                };

                let mut __roles = ::std::vec::Vec::new();
                #role_pushes

                let provider = ::std::sync::Arc::new(#provider_name { instance }) as ::std::sync::Arc<dyn ::ulo::spi::Provider>;
                ::std::result::Result::Ok(::ulo::spi::Registration::new(provider, __roles))
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
        .fields
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .collect();
    let value_reads = value_reads(dependencies.fields.iter().map(|(_, ty, tok)| (ty, tok)));

    let (dyn_factory_structs, factory_role_pushes) =
        generate_dyn_factories(struct_name, dependencies, enhancer_traits);

    let enhancer_preamble = if factory_role_pushes.is_empty() {
        quote! {}
    } else {
        quote! {
            let __has_request_deps = __deps.values().any(|registration|
                matches!(registration.instance.scope(), ::ulo::di::ProviderScope::Execution)
            );
            let __all_deps = ::std::sync::Arc::new(
                __deps.iter()
                    .map(|(k, registration)| (k.clone(), registration.instance.clone()))
                    .collect::<::ulo::FxHashMap<_, _>>()
            );
        }
    };

    let build_body = quote! {
        #enhancer_preamble
        let dependencies: ::ulo::FxHashMap<String, ::std::sync::Arc<dyn ::ulo::spi::Provider>> =
            __deps.into_iter().map(|(k, registration)| (k, registration.instance)).collect();
        let __provider: ::std::sync::Arc<dyn ::ulo::spi::Provider> =
            ::std::sync::Arc::new(#provider_name { dependencies }) as ::std::sync::Arc<dyn ::ulo::spi::Provider>;
        let mut __roles = ::std::vec::Vec::new();
        #factory_role_pushes
        ::std::result::Result::Ok(::ulo::spi::Registration::new(__provider, __roles))
    };

    quote! {
        #dyn_factory_structs

        pub struct #factory_name;

        #[::ulo::async_trait]
        impl ::ulo::spi::ProviderFactory for #factory_name {
            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn dependency_tokens(&self) -> Vec<String> {
                // A `#[new]` constructor supplies its own dependency tokens; else fall back to the
                // field-injection tokens.
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.tokens)()).unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            fn value_dependencies(&self) -> Vec<(String, &'static str)> {
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.values)()).unwrap_or_else(|| #value_reads)
            }

            async fn build(
                &self,
                __deps: ::ulo::FxHashMap<String, ::ulo::spi::Registration>,
            ) -> ::ulo::spi::BuildResult<::ulo::spi::Registration> {
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
        .fields
        .iter()
        .map(|(_, _, lookup_token_expr)| lookup_token_expr)
        .collect();
    let value_reads = value_reads(dependencies.fields.iter().map(|(_, ty, tok)| (ty, tok)));

    let (dyn_factory_structs, factory_role_pushes) =
        generate_dyn_factories(struct_name, dependencies, enhancer_traits);

    let has_enhancer_roles = !factory_role_pushes.is_empty();

    let build_body = if has_enhancer_roles {
        quote! {
            let __has_request_deps = __deps.values().any(|registration|
                matches!(registration.instance.scope(), ::ulo::di::ProviderScope::Execution)
            );
            let __all_deps = ::std::sync::Arc::new(
                __deps.iter()
                    .map(|(k, registration)| (k.clone(), registration.instance.clone()))
                    .collect::<::ulo::FxHashMap<_, _>>()
            );
            let dependencies: ::ulo::FxHashMap<String, ::std::sync::Arc<dyn ::ulo::spi::Provider>> =
                __deps.into_iter().map(|(k, registration)| (k, registration.instance)).collect();
            let mut __roles = ::std::vec::Vec::new();
            #factory_role_pushes
            ::std::result::Result::Ok(::ulo::spi::Registration::new(
                ::std::sync::Arc::new(#provider_name { dependencies }) as ::std::sync::Arc<dyn ::ulo::spi::Provider>,
                __roles,
            ))
        }
    } else {
        quote! {
            let dependencies: ::ulo::FxHashMap<String, ::std::sync::Arc<dyn ::ulo::spi::Provider>> =
                __deps.into_iter().map(|(k, registration)| (k, registration.instance)).collect();
            ::std::result::Result::Ok(::ulo::spi::Registration::new(
                ::std::sync::Arc::new(#provider_name { dependencies }) as ::std::sync::Arc<dyn ::ulo::spi::Provider>,
                ::std::vec::Vec::new(),
            ))
        }
    };

    quote! {
        #dyn_factory_structs

        pub struct #factory_name;

        #[::ulo::async_trait]
        impl ::ulo::spi::ProviderFactory for #factory_name {
            fn token(&self) -> String {
                ::ulo::di::token_of::<#struct_name>()
            }

            fn dependency_tokens(&self) -> Vec<String> {
                // A `#[new]` constructor supplies its own dependency tokens; else fall back to the
                // field-injection tokens.
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.tokens)()).unwrap_or_else(|| vec![#(#dependency_tokens),*])
            }

            fn value_dependencies(&self) -> Vec<(String, &'static str)> {
                use ::ulo::__construct::CtorBridge as _;
                <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.values)()).unwrap_or_else(|| #value_reads)
            }

            async fn build(
                &self,
                __deps: ::ulo::FxHashMap<String, ::ulo::spi::Registration>,
            ) -> ::ulo::spi::BuildResult<::ulo::spi::Registration> {
                #build_body
            }
        }
    }
}

/// The `#[inject]` fields' resolutions inside an enhancer builder's `__build_instance`, which each
/// role's `create` calls.
///
/// Unlike `field_resolutions`, which reads the dependency map and the execution its caller names,
/// this reads a captured `all_deps: Arc<FxHashMap<...>>` and resolves every field in the execution
/// `create` is handed.
fn generate_create_field_resolutions(
    dependencies: &DependencyInfo,
) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    for (field_name, full_type, lookup_token_expr) in &dependencies.fields {
        let field_name_str = field_name.to_string();
        let take = take_answer(
            full_type,
            quote! { __provider.resolve(__exec_ctx.clone()).await? },
        );
        resolutions.push(quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let __provider = all_deps.get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));
                #take
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
/// kind (the `ulo::__detect` value-probe inside `create()` compiles for any `T` — it yields the
/// coerced trait object for an implementor and never runs otherwise), and each role push is gated
/// by a `ulo::__detect` type-level probe over the concrete struct so only the kinds the type
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

    let struct_instantiation = struct_instantiation(struct_name, dependencies, &field_names);

    let deps_arc_ty = quote! {
        ::std::sync::Arc<::ulo::FxHashMap<
            String,
            ::std::sync::Arc<dyn ::ulo::spi::Provider>
        >>
    };

    // One shared builder per provider: it owns the (heavy) dependency-resolution + construction
    // logic once. Each `Dyn*Factory` impl is a thin shim that builds via this and value-probes the
    // result to its role trait object — so the field-resolution code isn't duplicated 11 times.
    let builder_struct_name = Ident::new(
        &format!("__Ulo{}EnhancerBuilder", struct_name),
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
                __exec_ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<#struct_name, ::ulo::di::ResolutionError> {
                // A `#[new]` constructor takes over construction; otherwise fall back to field
                // injection. Both thread the execution so execution-scoped sub-dependencies
                // resolve in it rather than in one of their own.
                use ::ulo::__construct::CtorBridge as _;
                if let ::std::option::Option::Some(__fut) =
                    <#struct_name>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.build)(&*self.all_deps, __exec_ctx.clone()))
                {
                    return __fut.await;
                }
                let all_deps = self.all_deps.clone();
                #(#field_resolutions)*
                ::std::result::Result::Ok(#struct_instantiation)
            }
        }
    };

    let mut impl_defs = Vec::new();
    let mut role_push_stmts = Vec::new();

    for kind in active_kinds {
        let spec = kind.spec();
        let trait_path = &spec.trait_path;
        let factory_trait_path = &spec.factory_trait;
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
                        use ::ulo::__detect::prelude::*;
                        // Built inside a live execution, which leaves nothing to refuse but a
                        // dependency of another type. The panic unwinds from `create`, which runs
                        // outside the recovery around `can_activate` and `intercept`, so the chain
                        // is not offered it as this enhancer's panic.
                        let instance = self
                            .__build_instance(#provider_ctx_variant(__ctx.clone()))
                            .await
                            .unwrap_or_else(|__error| panic!("{__error}"));
                        ::ulo::__detect::#value_probe(::std::sync::Arc::new(instance))
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
            if ::ulo::__detect::#type_probe::<#struct_name>(::std::marker::PhantomData).is() {
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
            use ::ulo::__detect::prelude::*;
            #(#role_push_stmts)*
        }
    };

    (struct_defs, role_pushes)
}

/// What `value_dependencies` reports, as one `ulo::__di::value_reads` call over the fields or
/// parameters `reads`: `(token, type name)` for each whose type reads a value, which its
/// `ulo::__di::Site` decides. `Arc<T>` and a collection read any binding.
pub(crate) fn value_reads<'a>(
    reads: impl Iterator<Item = (&'a Type, &'a TokenStream)>,
) -> TokenStream {
    let reads =
        reads.map(|(ty, token)| crate::shared::site::site_call(ty, quote! { value_read(#token) }));
    quote! { ::ulo::__di::value_reads([#(#reads),*]) }
}

/// How a field or parameter of type `ty` takes its value from a provider's erased `answer`, with
/// `__lookup_token` in scope, as its `ulo::__di::Site` decides: the shared instance or a value
/// wrapped for `Arc<T>`, the `Arc` a trait-object slot answers for `Arc<dyn Trait>`, the items for
/// a collection, and a value handed out as it is for any other type. Every resolver of an
/// `#[inject]` field or a `#[new]` parameter reads through this.
pub(crate) fn take_answer(ty: &Type, answer: TokenStream) -> TokenStream {
    let take = crate::shared::site::site_call(ty, quote! { take(#answer, &__lookup_token) });
    quote! { #take? }
}
