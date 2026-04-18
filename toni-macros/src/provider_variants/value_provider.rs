use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, Ident, Result, Token,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Enhancer type flags
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnhancerType {
    Guard,
    Interceptor,
    Pipe,
}

/// Parse provider_value! macro input
/// Syntax: provider_value!("TOKEN", value) or provider_value!(TOKEN, value)
/// Optional enhancers: provider_value!(TOKEN, value, guard) or provider_value!(TOKEN, value, guard, interceptor)
/// Optional type hint for string/const tokens with enhancers: provider_value!("TOKEN", value, Type, guard)
/// Note: Scope is NOT supported (values are always singleton)
pub struct ProviderValueInput {
    pub token: TokenType,
    pub value_expr: Expr,
    pub type_hint: Option<syn::Path>,
    pub enhancers: Vec<EnhancerType>,
    pub lifecycle: bool,
}

impl Parse for ProviderValueInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let value_expr: Expr = input.parse()?;

        // Parse optional type hint and enhancer flags
        let mut type_hint = None;
        let mut enhancers = Vec::new();
        let mut lifecycle = false;

        while input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break;
            }

            // Peek to determine if this is an enhancer keyword or type hint
            let lookahead = input.lookahead1();
            if lookahead.peek(Ident) {
                let ident: Ident = input.parse()?;
                let ident_str = ident.to_string();

                match ident_str.as_str() {
                    "guard" => enhancers.push(EnhancerType::Guard),
                    "interceptor" => enhancers.push(EnhancerType::Interceptor),
                    "pipe" => enhancers.push(EnhancerType::Pipe),
                    "lifecycle" => lifecycle = true,
                    _ => {
                        // Not an enhancer keyword - could be start of a type hint
                        if type_hint.is_none() && enhancers.is_empty() {
                            // Parse as path (might be multi-segment like my_mod::Type)
                            let mut path_segments = syn::punctuated::Punctuated::new();
                            path_segments.push(syn::PathSegment::from(ident));

                            // Check for additional path segments (::Type)
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
                                "Type hint must come before enhancer flags, or expected 'guard', 'interceptor', or 'pipe'",
                            ));
                        }
                    }
                }
            } else {
                return Err(lookahead.error());
            }
        }

        Ok(ProviderValueInput {
            token,
            value_expr,
            type_hint,
            enhancers,
            lifecycle,
        })
    }
}

/// Validate that enhancers can be used with the given token type.
fn validate_enhancers(
    token: &TokenType,
    type_hint: &Option<syn::Path>,
    enhancers: &[EnhancerType],
) -> Result<()> {
    if let TokenType::String(_) | TokenType::Const(_) = token {
        if !enhancers.is_empty() && type_hint.is_none() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "Enhancer support (guard/interceptor/pipe) for String or Const tokens requires a type hint. Use: provider_value!(\"TOKEN\", value, Type, guard)",
            ));
        }
    }
    Ok(())
}

/// Generate role-push statements to embed inside `build()` for value providers,
/// before the concrete `instance: Arc<T>` is boxed.
fn generate_value_role_pushes(enhancers: &[EnhancerType]) -> TokenStream {
    let mut pushes = Vec::new();
    for enhancer in enhancers {
        match enhancer {
            EnhancerType::Guard => pushes.push(quote! {
                __roles.push(toni::traits_helpers::ProviderRole::Guard(
                    toni::traits_helpers::GuardEntry::Ready(
                        instance.clone() as std::sync::Arc<dyn toni::traits_helpers::Guard>
                    )
                ));
            }),
            EnhancerType::Interceptor => pushes.push(quote! {
                __roles.push(toni::traits_helpers::ProviderRole::Interceptor(
                    toni::traits_helpers::InterceptorEntry::Ready(
                        instance.clone() as std::sync::Arc<dyn toni::traits_helpers::Interceptor>
                    )
                ));
            }),
            EnhancerType::Pipe => pushes.push(quote! {
                __roles.push(toni::traits_helpers::ProviderRole::Pipe(
                    toni::traits_helpers::PipeEntry::Ready(
                        instance.clone() as std::sync::Arc<dyn toni::traits_helpers::Pipe>
                    )
                ));
            }),
        }
    }
    quote! { #(#pushes)* }
}

pub fn handle_provider_value(input: TokenStream) -> Result<TokenStream> {
    let ProviderValueInput {
        token,
        value_expr,
        type_hint,
        enhancers,
        lifecycle,
    } = syn::parse2(input)?;

    if lifecycle && !enhancers.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "lifecycle cannot be combined with guard/interceptor/pipe enhancers",
        ));
    }

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

    validate_enhancers(&token, &type_hint, &enhancers)?;

    // Helper: emit a provider struct + factory that stores `instance: Arc<$instance_type>`.
    // Roles are built inside `build()` before boxing, so no downcast is needed.
    let make_concrete_provider = |instance_type: &TokenStream| {
        let role_pushes = generate_value_role_pushes(&enhancers);
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
            if !enhancers.is_empty() {
                let type_path = type_hint.as_ref().unwrap();
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

