use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, ExprClosure, Ident, Pat, Result, Token, Type,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;
use crate::shared::enhancer_emit::EnhancerKind;

pub struct ProviderFactoryInput {
    pub token: TokenType,
    pub factory_expr: Expr,
    pub scope: Option<String>,
    pub lifecycle: bool,
    pub type_hint: Option<syn::Path>,
}

impl Parse for ProviderFactoryInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let factory_expr: Expr = input.parse()?;

        let mut scope = None;
        let mut lifecycle = false;
        let mut type_hint = None;

        while input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break;
            }

            let lookahead = input.lookahead1();
            if lookahead.peek(Ident) {
                let ident: Ident = input.parse()?;

                if ident == "lifecycle" {
                    lifecycle = true;
                } else if ident == "scope" {
                    input.parse::<Token![=]>()?;
                    let scope_lit: syn::LitStr = input.parse()?;
                    scope = Some(scope_lit.value());
                } else if type_hint.is_none() {
                    // A type hint (possibly multi-segment / generic) — names the produced type so
                    // the request/transient path can gate enhancer detection on it.
                    let mut path_segments: syn::punctuated::Punctuated<
                        syn::PathSegment,
                        syn::token::PathSep,
                    > = syn::punctuated::Punctuated::new();
                    path_segments.push(syn::PathSegment::from(ident));
                    while input.peek(Token![::]) {
                        input.parse::<Token![::]>()?;
                        let segment: Ident = input.parse()?;
                        path_segments.push(syn::PathSegment::from(segment));
                    }
                    if input.peek(Token![<]) {
                        let _: syn::AngleBracketedGenericArguments = input.parse()?;
                    }
                    type_hint = Some(syn::Path {
                        leading_colon: None,
                        segments: path_segments,
                    });
                } else {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "expected `scope = \"...\"`, `lifecycle`, or a single type hint",
                    ));
                }
            } else {
                return Err(lookahead.error());
            }
        }

        Ok(ProviderFactoryInput {
            token,
            factory_expr,
            scope,
            lifecycle,
            type_hint,
        })
    }
}

fn extract_closure_deps(closure: &ExprClosure) -> Vec<(syn::Ident, Type)> {
    let mut deps = Vec::new();
    for input in &closure.inputs {
        if let Pat::Type(pat_type) = input {
            if let Pat::Ident(pat_ident) = &*pat_type.pat {
                deps.push((pat_ident.ident.clone(), (*pat_type.ty).clone()));
            }
        }
    }
    deps
}

fn is_async_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Async(_) => true,
        Expr::Closure(closure) => closure.asyncness.is_some(),
        _ => false,
    }
}

/// Generates the caching provider struct definition and the build() body.
/// Roles are built inside build() before boxing, so no downcast is ever needed.
fn generate_caching_provider(
    provider_name: &Ident,
    token_expr: &TokenStream,
    scope_expr: &TokenStream,
    factory_expr: &Expr,
    dep_resolutions: &[TokenStream],
    param_names: &[&syn::Ident],
    lifecycle: bool,
) -> (TokenStream, TokenStream) {
    let is_async = is_async_expr(factory_expr);
    let has_deps = !dep_resolutions.is_empty();

    let type_bounds = if lifecycle {
        quote! { toni::traits_helpers::Provider + 'static }
    } else {
        quote! { Clone + Send + Sync + 'static }
    };

    let factory_call = if is_async {
        quote! { factory(#(#param_names),*).await }
    } else {
        quote! { factory(#(#param_names),*) }
    };

    let struct_init_code = if is_async || has_deps {
        quote! {
            let factory = #factory_expr;
            let instance_raw = async {
                #(#dep_resolutions)*
                #factory_call
            }.await;
            let instance = std::sync::Arc::new(instance_raw);
        }
    } else {
        quote! {
            let factory = #factory_expr;
            let instance = std::sync::Arc::new(factory());
        }
    };

    let execute_body = if lifecycle {
        quote! { self.instance.execute(_params, _ctx).await }
    } else {
        quote! { Box::new((*self.instance).clone()) }
    };

    let extra_methods = if lifecycle {
        quote! {
            async fn on_module_init(&self) {
                self.instance.on_module_init().await;
            }
            async fn on_application_bootstrap(&self) {
                self.instance.on_application_bootstrap().await;
            }
            async fn on_module_destroy(&self) {
                self.instance.on_module_destroy().await;
            }
            async fn before_application_shutdown(&self, signal: Option<String>) {
                self.instance.before_application_shutdown(signal).await;
            }
            async fn on_application_shutdown(&self, signal: Option<String>) {
                self.instance.on_application_shutdown(signal).await;
            }
        }
    } else {
        quote! {}
    };

    let struct_def = quote! {
        struct #provider_name<__T> {
            deps: std::sync::Arc<toni::FxHashMap<
                String,
                std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>,
            >>,
            instance: std::sync::Arc<__T>,
        }

        #[toni::async_trait]
        impl<__T: #type_bounds> toni::traits_helpers::Provider for #provider_name<__T> {
            fn get_token(&self) -> String { #token_expr }
            fn get_token_factory(&self) -> String { #token_expr }
            fn get_scope(&self) -> toni::ProviderScope { #scope_expr }

            async fn execute(
                &self,
                _params: Vec<Box<dyn std::any::Any + Send>>,
                _ctx: toni::ProviderContext<'_>,
            ) -> Box<dyn std::any::Any + Send> {
                #execute_body
            }

            #extra_methods
        }
    };

    // Roles are detected from the built value's type via the shared autoref probes — the same
    // path the #[injectable] singleton factory uses. `instance: Arc<__T>` is concrete here, so the
    // probes resolve.
    let role_pushes = crate::shared::enhancer_emit::value_probe_detection();

    let build_body = quote! {
        #struct_init_code

        let mut __roles = std::vec::Vec::new();
        #role_pushes

        let __provider = std::sync::Arc::new(Box::new(#provider_name {
            deps: std::sync::Arc::new(_dependencies),
            instance,
        }) as Box<dyn toni::traits_helpers::Provider>);
        toni::traits_helpers::Injectable::new(__provider, __roles)
    };

    (struct_def, build_body)
}

pub fn handle_provider_factory(input: TokenStream) -> Result<TokenStream> {
    let ProviderFactoryInput {
        token,
        factory_expr,
        scope,
        lifecycle,
        type_hint,
    } = syn::parse2(input)?;

    let scope_expr = match scope.as_deref() {
        Some("request") => quote! { toni::ProviderScope::Request },
        Some("singleton") => quote! { toni::ProviderScope::Singleton },
        Some("transient") => quote! { toni::ProviderScope::Transient },
        None => quote! { toni::ProviderScope::Singleton },
        Some(other) => {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "Invalid scope '{}'. Expected 'singleton', 'request', or 'transient'",
                    other
                ),
            ));
        }
    };

    if lifecycle && matches!(scope.as_deref(), Some("request") | Some("transient")) {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "lifecycle is only compatible with singleton scope",
        ));
    }

    // For Type tokens the type is already known; for String/Const the caller may pass a hint, which
    // the non-caching path needs to gate enhancer detection.
    let effective_type_hint = type_hint.or_else(|| {
        if let TokenType::Type(path) = &token {
            Some(path.clone())
        } else {
            None
        }
    });

    let token_expr = token.to_token_expr();
    let is_async = is_async_expr(&factory_expr);

    let deps = if let Expr::Closure(ref closure) = factory_expr {
        extract_closure_deps(closure)
    } else {
        Vec::new()
    };

    let dep_resolutions: Vec<_> = deps
        .iter()
        .map(|(param_name, param_type)| {
            let type_token = quote! { std::any::type_name::<#param_type>().to_string() };
            quote! {
                let #param_name = {
                    let provider = _dependencies
                        .get(&#type_token)
                        .expect(&format!("Dependency not found: {}", #type_token));
                    let instance = provider.execute(vec![], toni::ProviderContext::None).await;
                    *instance
                        .downcast::<#param_type>()
                        .expect(&format!("Failed to downcast {}", #type_token))
                };
            }
        })
        .collect();

    let param_names: Vec<_> = deps.iter().map(|(name, _)| name).collect();

    let factory_invocation = if deps.is_empty() {
        if is_async {
            quote! { { let result = factory().await; Box::new(result) as Box<dyn std::any::Any + Send> } }
        } else {
            quote! { { let result = factory(); Box::new(result) as Box<dyn std::any::Any + Send> } }
        }
    } else if is_async {
        quote! { { #(#dep_resolutions)* let result = factory(#(#param_names),*).await; Box::new(result) as Box<dyn std::any::Any + Send> } }
    } else {
        quote! { { #(#dep_resolutions)* let result = factory(#(#param_names),*); Box::new(result) as Box<dyn std::any::Any + Send> } }
    };

    let dep_tokens: Vec<_> = deps
        .iter()
        .map(|(_, param_type)| quote! { std::any::type_name::<#param_type>().to_string() })
        .collect();

    let token_display = token.display_name();
    let sanitized_name = token_display.replace(['\"', ' ', '-', '.', ':', '/'], "_");
    let factory_name = format_ident!("__ToniFactoryProviderFactory_{}", sanitized_name);
    let provider_name = format_ident!("__ToniFactoryProvider_{}", sanitized_name);

    let needs_caching = !matches!(scope.as_deref(), Some("request") | Some("transient"));

    // The non-caching path registers enhancers from the produced value's type. Since registration
    // is decided in build() before any value exists, it needs the concrete type by name: prefer the
    // closure's written `-> T`, else the explicit hint, else a `Type` token. None ⟹ no enhancer
    // detection (a type-less factory closure can't be gated).
    let closure_return_type = if let Expr::Closure(ref closure) = factory_expr {
        match &closure.output {
            syn::ReturnType::Type(_, ty) => match &**ty {
                Type::Path(tp) => Some(tp.path.clone()),
                _ => None,
            },
            syn::ReturnType::Default => None,
        }
    } else {
        None
    };
    let noncaching_type = closure_return_type.or_else(|| effective_type_hint.clone());

    let (provider_struct_def, build_body) = if !needs_caching {
        let (dyn_factory_structs, factory_role_pushes) = match &noncaching_type {
            Some(ty) => generate_noncaching_factory_structs(
                &sanitized_name,
                &factory_expr,
                &dep_resolutions,
                &param_names,
                is_async,
                ty,
            ),
            None => (quote! {}, quote! {}),
        };

        let has_enhancer_roles = !factory_role_pushes.is_empty();

        let non_caching_body = if has_enhancer_roles {
            quote! {
                #dyn_factory_structs

                struct FactoryProviderWithDeps {
                    deps: std::sync::Arc<toni::FxHashMap<
                        String,
                        std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>,
                    >>,
                }

                #[toni::async_trait]
                impl toni::traits_helpers::Provider for FactoryProviderWithDeps {
                    fn get_token(&self) -> String { #token_expr }
                    fn get_token_factory(&self) -> String { #token_expr }
                    fn get_scope(&self) -> toni::ProviderScope { #scope_expr }

                    async fn execute(
                        &self,
                        _params: Vec<Box<dyn std::any::Any + Send>>,
                        _ctx: toni::ProviderContext<'_>,
                    ) -> Box<dyn std::any::Any + Send> {
                        let _dependencies = &self.deps;
                        let factory = #factory_expr;
                        #factory_invocation
                    }
                }

                let __all_deps = std::sync::Arc::new(_dependencies);
                let mut __roles = std::vec::Vec::new();
                #factory_role_pushes
                toni::traits_helpers::Injectable::new(
                    std::sync::Arc::new(Box::new(FactoryProviderWithDeps {
                        deps: __all_deps,
                    }) as Box<dyn toni::traits_helpers::Provider>),
                    __roles,
                )
            }
        } else {
            quote! {
                struct FactoryProviderWithDeps {
                    deps: std::sync::Arc<toni::FxHashMap<
                        String,
                        std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>,
                    >>,
                }

                #[toni::async_trait]
                impl toni::traits_helpers::Provider for FactoryProviderWithDeps {
                    fn get_token(&self) -> String { #token_expr }
                    fn get_token_factory(&self) -> String { #token_expr }
                    fn get_scope(&self) -> toni::ProviderScope { #scope_expr }

                    async fn execute(
                        &self,
                        _params: Vec<Box<dyn std::any::Any + Send>>,
                        _ctx: toni::ProviderContext<'_>,
                    ) -> Box<dyn std::any::Any + Send> {
                        let _dependencies = &self.deps;
                        let factory = #factory_expr;
                        #factory_invocation
                    }
                }

                toni::traits_helpers::Injectable::new(
                    std::sync::Arc::new(Box::new(FactoryProviderWithDeps {
                        deps: std::sync::Arc::new(_dependencies),
                    }) as Box<dyn toni::traits_helpers::Provider>),
                    std::vec::Vec::new(),
                )
            }
        };
        (quote! {}, non_caching_body)
    } else {
        generate_caching_provider(
            &provider_name,
            &token_expr,
            &scope_expr,
            &factory_expr,
            &dep_resolutions,
            &param_names,
            lifecycle,
        )
    };

    let expanded = quote! {
        {
            #provider_struct_def

            struct #factory_name;

            #[toni::async_trait]
            impl toni::traits_helpers::ProviderFactory for #factory_name {
                fn get_token(&self) -> String {
                    #token_expr
                }

                fn get_dependencies(&self) -> Vec<String> {
                    vec![#(#dep_tokens),*]
                }

                async fn build(
                    &self,
                    __deps: toni::FxHashMap<String, toni::traits_helpers::Injectable>,
                ) -> toni::traits_helpers::Injectable {
                    let _dependencies: toni::FxHashMap<String, std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>> =
                        __deps.into_iter().map(|(k, inj)| (k, inj.instance)).collect();
                    #build_body
                }
            }

            #factory_name
        }
    };

    Ok(expanded)
}

/// Generates the per-request enhancer factories for the non-caching (`request` / `transient`)
/// path of `provider_factory!`, detecting roles from the produced value's type — no marker.
///
/// `effective_type` is the closure's concrete output type (from a written `-> T`, a `Type` token,
/// or an explicit type hint). It's required because the registration decision happens in `build()`,
/// before any value exists, so the type-level probe needs a name to gate on. When it's `None` the
/// caller skips enhancer registration entirely (a type-less factory closure can't be auto-probed).
///
/// One `Dyn*Factory` is emitted per enhancer kind; each `create()` re-invokes the closure and
/// value-probes the fresh result (compiles for any output type via the `None` fallback, and only
/// ever runs for a kind whose registration the type-probe admitted, so the `expect` can't fire).
/// `requires_http_parts()` is always `false` — dep resolution uses `ProviderContext::None`, as the
/// non-caching provider's `execute()` does.
///
/// Returns `(struct_defs, role_push_stmts)`; role pushes assume `__all_deps: Arc<FxHashMap<...>>`.
fn generate_noncaching_factory_structs(
    sanitized_name: &str,
    factory_expr: &Expr,
    dep_resolutions: &[TokenStream],
    param_names: &[&syn::Ident],
    is_async: bool,
    effective_type: &syn::Path,
) -> (TokenStream, TokenStream) {
    let deps_arc_ty = quote! {
        std::sync::Arc<toni::FxHashMap<
            String,
            std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>
        >>
    };

    // The create() body resolves deps (same as execute()) then wraps in Arc.
    let create_call = if dep_resolutions.is_empty() {
        if is_async {
            quote! { factory().await }
        } else {
            quote! { factory() }
        }
    } else if is_async {
        quote! { { #(#dep_resolutions)* factory(#(#param_names),*).await } }
    } else {
        quote! { { #(#dep_resolutions)* factory(#(#param_names),*) } }
    };

    let mut struct_defs = Vec::new();
    let mut role_push_stmts = Vec::new();

    for kind in EnhancerKind::all() {
        let spec = kind.spec();
        let struct_name = format_ident!(
            "__ToniFactory{}DynFactory_{}",
            spec.factory_suffix,
            sanitized_name
        );
        let trait_path = &spec.trait_path;
        let entry_path = &spec.entry_path;
        let role_variant = &spec.role_variant;
        let dyn_factory_trait = &spec.dyn_factory_trait;
        let value_probe = format_ident!("{}Probe", spec.factory_suffix);
        let type_probe = format_ident!("{}TypeProbe", spec.factory_suffix);

        struct_defs.push(quote! {
            struct #struct_name {
                all_deps: #deps_arc_ty,
            }

            impl #dyn_factory_trait for #struct_name {
                fn requires_http_parts(&self) -> bool { false }

                fn create<'a>(
                    &'a self,
                    _request_parts: Option<&'a toni::http_helpers::RequestPart>,
                ) -> std::pin::Pin<Box<dyn std::future::Future<
                    Output = std::sync::Arc<dyn #trait_path + Send + Sync>
                > + Send + 'a>> {
                    let all_deps = self.all_deps.clone();
                    std::boxed::Box::pin(async move {
                        use toni::__detect::prelude::*;
                        let _dependencies = &all_deps;
                        let factory = #factory_expr;
                        let result = #create_call;
                        toni::__detect::#value_probe(std::sync::Arc::new(result))
                            .detect()
                            .expect("enhancer factory registered only when the produced type implements the role")
                            as std::sync::Arc<dyn #trait_path + Send + Sync>
                    })
                }
            }
        });

        role_push_stmts.push(quote! {
            if toni::__detect::#type_probe::<#effective_type>(std::marker::PhantomData).is() {
                __roles.push(#role_variant(
                    #entry_path::Factory(std::sync::Arc::new(#struct_name { all_deps: __all_deps.clone() }))
                ));
            }
        });
    }

    let role_pushes = quote! {
        {
            use toni::__detect::prelude::*;
            #(#role_push_stmts)*
        }
    };

    (quote! { #(#struct_defs)* }, role_pushes)
}
