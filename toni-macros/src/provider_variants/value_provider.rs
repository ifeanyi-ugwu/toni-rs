use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, Ident, Result, Token,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Parse provider_value! macro input.
///
/// Syntax: `provider_value!("TOKEN", value)` or `provider_value!(TOKEN, value)`.
///
/// An optional type hint — `provider_value!("TOKEN", value, Type)` — stores the value as a
/// concrete `Arc<Type>` rather than type-erased, which lets the framework auto-detect any enhancer
/// roles the type implements (`Guard<C>`, etc.). For a `Type` token the hint is implicit. Values
/// are always singleton; scope is not supported.
pub struct ProviderValueInput {
    pub token: TokenType,
    pub value_expr: Expr,
    pub type_hint: Option<syn::Path>,
    pub lifecycle: bool,
}

impl Parse for ProviderValueInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let value_expr: Expr = input.parse()?;

        let mut type_hint = None;
        let mut lifecycle = false;

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
                } else if type_hint.is_none() {
                    // A path (possibly multi-segment, e.g. `my_mod::Type`) — the storage/probe type.
                    let mut path_segments = syn::punctuated::Punctuated::new();
                    path_segments.push(syn::PathSegment::from(ident));
                    while input.peek(Token![::]) {
                        input.parse::<Token![::]>()?;
                        let segment: Ident = input.parse()?;
                        path_segments.push(syn::PathSegment::from(segment));
                    }
                    type_hint = Some(syn::Path {
                        leading_colon: None,
                        segments: path_segments,
                    });
                } else {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "expected a single type hint or `lifecycle`",
                    ));
                }
            } else {
                return Err(lookahead.error());
            }
        }

        Ok(ProviderValueInput {
            token,
            value_expr,
            type_hint,
            lifecycle,
        })
    }
}

/// Generate role-push statements to embed inside `build()` for value providers, before the
/// concrete `instance: Arc<T>` is boxed. Roles are detected from the value's type via the shared
/// autoref probes — same path the `#[injectable]` and `provider_factory!` singletons use.
fn generate_value_role_pushes() -> TokenStream {
    crate::shared::enhancer_emit::value_probe_detection()
}

pub fn handle_provider_value(input: TokenStream) -> Result<TokenStream> {
    let ProviderValueInput {
        token,
        value_expr,
        type_hint,
        lifecycle,
    } = syn::parse2(input)?;

    // Generate token expression for runtime
    let token_expr = token.to_token_expr();

    // Generate unique struct names based on token for this specific provider instance
    let token_display = token.display_name();
    let sanitized_name = token_display.replace(['\"', ' ', '-', '.', ':', '/'], "_");
    let provider_name = format_ident!("__ToniValueProvider_{}", sanitized_name);
    let factory_name = format_ident!("__ToniValueProviderFactory_{}", sanitized_name);

    if lifecycle {
        let expanded = quote! {
            {
                struct #provider_name {
                    instance: std::sync::Arc<Box<dyn toni::traits_helpers::Provider>>,
                }

                struct #factory_name;

                #[toni::async_trait]
                impl toni::traits_helpers::Provider for #provider_name {
                    fn get_token(&self) -> String {
                        #token_expr
                    }

                    fn get_token_factory(&self) -> String {
                        #token_expr
                    }

                    fn get_scope(&self) -> toni::ProviderScope {
                        toni::ProviderScope::Singleton
                    }

                    async fn execute(
                        &self,
                        _params: Vec<Box<dyn std::any::Any + Send>>,
                        _ctx: toni::ProviderContext<'_>,
                    ) -> Box<dyn std::any::Any + Send> {
                        self.instance.execute(_params, _ctx).await
                    }

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

                #[toni::async_trait]
                impl toni::traits_helpers::ProviderFactory for #factory_name {
                    fn get_token(&self) -> String {
                        #token_expr
                    }

                    async fn build(
                        &self,
                        _deps: toni::FxHashMap<String, toni::traits_helpers::Injectable>,
                    ) -> toni::traits_helpers::Injectable {
                        let instance = std::sync::Arc::new(
                            Box::new(#value_expr) as Box<dyn toni::traits_helpers::Provider>
                        );
                        toni::traits_helpers::Injectable::new(
                            std::sync::Arc::new(
                                Box::new(#provider_name { instance }) as Box<dyn toni::traits_helpers::Provider>
                            ),
                            std::vec::Vec::new(),
                        )
                    }
                }

                #factory_name
            }
        };
        return Ok(expanded);
    }

    // Helper: emit a provider struct + factory that stores `instance: Arc<$instance_type>`.
    // Roles are built inside `build()` before boxing, so no downcast is needed.
    let make_concrete_provider = |instance_type: &TokenStream| {
        let role_pushes = generate_value_role_pushes();
        quote! {
            {
                #[derive(Clone)]
                struct #provider_name {
                    instance: std::sync::Arc<#instance_type>,
                }

                struct #factory_name;

                #[toni::async_trait]
                impl toni::traits_helpers::Provider for #provider_name {
                    fn get_token(&self) -> String { #token_expr }
                    fn get_token_factory(&self) -> String { #token_expr }
                    fn get_scope(&self) -> toni::ProviderScope { toni::ProviderScope::Singleton }

                    async fn execute(
                        &self,
                        _params: Vec<Box<dyn std::any::Any + Send>>,
                        _ctx: toni::ProviderContext<'_>,
                    ) -> Box<dyn std::any::Any + Send> {
                        Box::new((*self.instance).clone())
                    }
                }

                #[toni::async_trait]
                impl toni::traits_helpers::ProviderFactory for #factory_name {
                    fn get_token(&self) -> String { #token_expr }

                    async fn build(
                        &self,
                        _deps: toni::FxHashMap<String, toni::traits_helpers::Injectable>,
                    ) -> toni::traits_helpers::Injectable {
                        let instance = std::sync::Arc::new(#value_expr);
                        let mut __roles = std::vec::Vec::new();
                        #role_pushes
                        toni::traits_helpers::Injectable::new(
                            std::sync::Arc::new(
                                Box::new(#provider_name { instance }) as Box<dyn toni::traits_helpers::Provider>
                            ),
                            __roles,
                        )
                    }
                }

                #factory_name
            }
        }
    };

    let expanded = match &token {
        TokenType::Type(path) => {
            let instance_type = quote! { #path };
            make_concrete_provider(&instance_type)
        }

        TokenType::String(_) | TokenType::Const(_) => {
            if let Some(type_path) = type_hint.as_ref() {
                // A hint means "store concretely so roles can be auto-detected".
                let instance_type = quote! { #type_path };
                make_concrete_provider(&instance_type)
            } else {
                quote! {
                    {
                        struct #provider_name {
                            get_value: std::sync::Arc<dyn Fn() -> Box<dyn std::any::Any + Send> + Send + Sync>,
                        }

                        struct #factory_name;

                        #[toni::async_trait]
                        impl toni::traits_helpers::Provider for #provider_name {
                            fn get_token(&self) -> String { #token_expr }
                            fn get_token_factory(&self) -> String { #token_expr }
                            fn get_scope(&self) -> toni::ProviderScope { toni::ProviderScope::Singleton }

                            async fn execute(
                                &self,
                                _params: Vec<Box<dyn std::any::Any + Send>>,
                                _ctx: toni::ProviderContext<'_>,
                            ) -> Box<dyn std::any::Any + Send> {
                                (self.get_value)()
                            }
                        }

                        #[toni::async_trait]
                        impl toni::traits_helpers::ProviderFactory for #factory_name {
                            fn get_token(&self) -> String { #token_expr }

                            async fn build(
                                &self,
                                _deps: toni::FxHashMap<String, toni::traits_helpers::Injectable>,
                            ) -> toni::traits_helpers::Injectable {
                                let value = std::sync::Arc::new(#value_expr);
                                let get_value = std::sync::Arc::new(move || {
                                    Box::new((*value).clone()) as Box<dyn std::any::Any + Send>
                                });
                                toni::traits_helpers::Injectable::new(
                                    std::sync::Arc::new(
                                        Box::new(#provider_name { get_value }) as Box<dyn toni::traits_helpers::Provider>
                                    ),
                                    std::vec::Vec::new(),
                                )
                            }
                        }

                        #factory_name
                    }
                }
            }
        }
    };

    Ok(expanded)
}
