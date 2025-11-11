use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, Result, Token,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Parse provider_value! macro input
/// Syntax: provider_value!("TOKEN", value) or provider_value!(TOKEN, value)
pub struct ProviderValueInput {
    pub token: TokenType,
    pub value_expr: Expr,
}

impl Parse for ProviderValueInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let value_expr: Expr = input.parse()?;

        Ok(ProviderValueInput { token, value_expr })
    }
}

pub fn handle_provider_value(input: TokenStream) -> Result<TokenStream> {
    let ProviderValueInput { token, value_expr } = syn::parse2(input)?;

    // Generate token expression for runtime
    let token_expr = token.to_token_expr();

    // Generate unique struct names based on token for this specific provider instance
    let token_display = token.display_name();
    let sanitized_name = token_display.replace(['\"', ' ', '-', '.', ':', '/'], "_");
    let provider_name = format_ident!("__ToniValueProvider_{}", sanitized_name);
    let manager_name = format_ident!("__ToniValueProviderManager_{}", sanitized_name);

    // Generate the provider struct and implementation
    // This entire block will be the expression that evaluates to the Manager type
    let expanded = quote! {
        {
            // Value provider struct that wraps the actual value
            #[derive(Clone)]
            struct #provider_name {
                instance: std::sync::Arc<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
            }

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
                    toni::ProviderScope::Singleton
                }

                async fn execute(
                    &self,
                    _params: Vec<Box<dyn std::any::Any + Send>>,
                    _req: Option<&toni::HttpRequest>,
                ) -> Box<dyn std::any::Any + Send> {
                    Box::new((*self.instance).clone())
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

                    // Create the value instance
                    let value = #value_expr;
                    let arc_value: std::sync::Arc<dyn std::any::Any + Send + Sync> =
                        std::sync::Arc::new(value);
                    let instance = std::sync::Arc::new(arc_value);

                    // Create the provider wrapper
                    let provider_wrapper = #provider_name { instance };

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
