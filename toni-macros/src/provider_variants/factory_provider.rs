use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    Expr, Result, Token,
};

use crate::shared::TokenType;

/// Parse provider_factory! macro input
/// Syntax: provider_factory!("TOKEN", factory_fn) or provider_factory!(TOKEN, factory_fn)
/// where factory_fn can be:
/// - || { value } - sync factory
/// - async || { value } - async factory
pub struct ProviderFactoryInput {
    pub token: TokenType,
    pub factory_expr: Expr,
}

impl Parse for ProviderFactoryInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let factory_expr: Expr = input.parse()?;

        Ok(ProviderFactoryInput {
            token,
            factory_expr,
        })
    }
}

/// Check if an expression is an async closure or async block
fn is_async_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Async(_) => true,
        Expr::Closure(closure) => closure.asyncness.is_some(),
        _ => false,
    }
}

pub fn handle_provider_factory(input: TokenStream) -> Result<TokenStream> {
    let ProviderFactoryInput {
        token,
        factory_expr,
    } = syn::parse2(input)?;

    // Generate token expression for runtime
    let token_expr = token.to_token_expr();

    // Detect if factory is async
    let is_async = is_async_expr(&factory_expr);

    // Generate unique struct names based on token
    let token_display = token.display_name();
    let sanitized_name = token_display.replace(['\"', ' ', '-', '.', ':', '/'], "_");
    let provider_name = format_ident!("__ToniFactoryProvider_{}", sanitized_name);
    let manager_name = format_ident!("__ToniFactoryProviderManager_{}", sanitized_name);

    // Generate the appropriate factory invocation based on async detection
    let factory_invocation = if is_async {
        quote! {
            {
                let result = factory().await;
                Box::new(result) as Box<dyn std::any::Any + Send>
            }
        }
    } else {
        quote! {
            {
                let result = factory();
                Box::new(result) as Box<dyn std::any::Any + Send>
            }
        }
    };

    // Generate the provider struct and implementation
    let expanded = quote! {
        {
            // Factory provider struct
            #[derive(Clone)]
            struct #provider_name;

            // Manager struct for Provider trait implementation
            struct #manager_name;

            // Implement ProviderTrait for the provider wrapper
            #[toni::async_trait]
            impl toni::traits_helpers::ProviderTrait for #provider_name {
                fn get_token(&self) -> String {
                    #token_expr
                }

                fn get_token_manager(&self) -> String {
                    #token_expr
                }

                fn get_scope(&self) -> toni::ProviderScope {
                    // Factories are transient by default (create new instance each time)
                    toni::ProviderScope::Transient
                }

                async fn execute(
                    &self,
                    _params: Vec<Box<dyn std::any::Any + Send>>,
                    _req: Option<&toni::HttpRequest>,
                ) -> Box<dyn std::any::Any + Send> {
                    // Call the factory function
                    let factory = #factory_expr;
                    #factory_invocation
                }
            }

            // Implement Provider trait for the manager (used by module system)
            #[toni::async_trait]
            impl toni::traits_helpers::Provider for #manager_name {
                async fn get_all_providers(
                    &self,
                    _dependencies: &toni::FxHashMap<
                        String,
                        std::sync::Arc<Box<dyn toni::traits_helpers::ProviderTrait>>,
                    >,
                ) -> toni::FxHashMap<
                    String,
                    std::sync::Arc<Box<dyn toni::traits_helpers::ProviderTrait>>,
                > {
                    let mut providers = toni::FxHashMap::default();

                    // Create the provider wrapper
                    let provider_wrapper = #provider_name;

                    // Register the provider with its token
                    let token = #token_expr;
                    providers.insert(
                        token,
                        std::sync::Arc::new(
                            Box::new(provider_wrapper) as Box<dyn toni::traits_helpers::ProviderTrait>
                        ),
                    );

                    providers
                }

                fn get_name(&self) -> String {
                    #token_expr
                }

                fn get_token(&self) -> String {
                    #token_expr
                }

                fn get_dependencies(&self) -> Vec<String> {
                    Vec::new()
                }
            }

            // Return the manager instance
            #manager_name
        }
    };

    Ok(expanded)
}
