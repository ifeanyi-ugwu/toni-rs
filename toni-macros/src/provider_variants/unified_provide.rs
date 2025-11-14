use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, ExprCall, ExprClosure, ExprLit, ExprPath, Result, Token, Type,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Unified provider macro that supports all provider variants with auto-detection
///
/// Syntax:
/// - `provide!("TOKEN", value)` - Auto-detect as value provider
/// - `provide!("TOKEN", |deps| ...)` - Auto-detect as factory provider
/// - `provide!("TOKEN", existing(Target))` - Explicit alias provider
/// - `provide!("TOKEN", provider(Type))` - Explicit token provider
/// - `provide!("TOKEN", value(expr))` - Explicit value provider
/// - `provide!("TOKEN", factory(closure))` - Explicit factory provider
pub struct ProvideInput {
    pub token: TokenType,
    pub variant: ProviderVariant,
}

/// The detected provider variant
pub enum ProviderVariant {
    /// Value provider - for constants and expressions
    Value(Expr),
    /// Factory provider - for closures and factory functions
    Factory(Expr),
    /// Alias provider - reference to existing provider
    Alias(TokenType),
    /// Token provider - register type under custom token
    TokenProvider(Type),
}

impl Parse for ProvideInput {
    fn parse(input: ParseStream) -> Result<Self> {
        // Parse token (first argument)
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;

        // Parse the value expression (second argument)
        let expr: Expr = input.parse()?;

        // Detect the provider variant
        let variant = detect_provider_variant(expr)?;

        Ok(ProvideInput { token, variant })
    }
}

/// Detect the provider variant from the expression
fn detect_provider_variant(expr: Expr) -> Result<ProviderVariant> {
    // Check if it's a marker function call: existing(...), provider(...), value(...), factory(...)
    if let Expr::Call(ExprCall { func, args, .. }) = &expr {
        if let Expr::Path(ExprPath { path, .. }) = &**func {
            if let Some(segment) = path.segments.last() {
                let func_name = segment.ident.to_string();

                match func_name.as_str() {
                    // Marker: existing(TokenType) -> Alias
                    "existing" => {
                        if args.len() != 1 {
                            return Err(syn::Error::new_spanned(
                                expr,
                                "existing() expects exactly one argument",
                            ));
                        }

                        let arg = &args[0];
                        // Try to parse as TokenType (could be Type, String, or Const)
                        let token_type = parse_token_type_from_expr(arg)?;
                        return Ok(ProviderVariant::Alias(token_type));
                    }

                    // Marker: provider(Type) -> TokenProvider
                    "provider" => {
                        if args.len() != 1 {
                            return Err(syn::Error::new_spanned(
                                expr,
                                "provider() expects exactly one argument",
                            ));
                        }

                        let arg = &args[0];
                        // Must be a Type
                        let provider_type = parse_type_from_expr(arg)?;
                        return Ok(ProviderVariant::TokenProvider(provider_type));
                    }

                    // Marker: value(expr) -> Value (explicit)
                    "value" => {
                        if args.len() != 1 {
                            return Err(syn::Error::new_spanned(
                                expr,
                                "value() expects exactly one argument",
                            ));
                        }

                        let value_expr = args[0].clone();
                        return Ok(ProviderVariant::Value(value_expr));
                    }

                    // Marker: factory(closure) -> Factory (explicit)
                    "factory" => {
                        if args.len() != 1 {
                            return Err(syn::Error::new_spanned(
                                expr,
                                "factory() expects exactly one argument",
                            ));
                        }

                        let factory_expr = args[0].clone();
                        return Ok(ProviderVariant::Factory(factory_expr));
                    }

                    _ => {
                        // Not a marker function, fall through to auto-detection
                    }
                }
            }
        }
    }

    // Auto-detect based on expression type
    match &expr {
        // Closures -> Factory
        Expr::Closure(ExprClosure { .. }) => Ok(ProviderVariant::Factory(expr)),

        // Async blocks -> Factory
        Expr::Async(_) => Ok(ProviderVariant::Factory(expr)),

        // Literals -> Value
        Expr::Lit(ExprLit { .. }) => Ok(ProviderVariant::Value(expr)),

        // Everything else -> Value (expressions, function calls, etc.)
        _ => Ok(ProviderVariant::Value(expr)),
    }
}

/// Parse a TokenType from an expression
/// Supports: Type paths, String literals, Const identifiers
fn parse_token_type_from_expr(expr: &Expr) -> Result<TokenType> {
    match expr {
        // String literal: "TOKEN"
        Expr::Lit(ExprLit {
            lit: syn::Lit::Str(lit_str),
            ..
        }) => Ok(TokenType::String(lit_str.value())),

        // Type path or const: AuthService or API_KEY
        Expr::Path(expr_path) => {
            let path = &expr_path.path;

            // Check if it's a SCREAMING_SNAKE_CASE const
            if let Some(segment) = path.segments.last() {
                let ident_str = segment.ident.to_string();
                if is_screaming_snake_case(&ident_str) {
                    return Ok(TokenType::Const(path.clone()));
                }
            }

            // Otherwise treat as Type
            Ok(TokenType::Type(path.clone()))
        }

        _ => Err(syn::Error::new_spanned(
            expr,
            "Expected a type, string literal, or const identifier",
        )),
    }
}

/// Parse a Type from an expression
fn parse_type_from_expr(expr: &Expr) -> Result<Type> {
    match expr {
        Expr::Path(expr_path) => {
            let type_path = Type::Path(syn::TypePath {
                qself: expr_path.qself.clone(),
                path: expr_path.path.clone(),
            });
            Ok(type_path)
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "Expected a type (e.g., DatabaseService)",
        )),
    }
}

/// Check if a string is SCREAMING_SNAKE_CASE (const identifier)
fn is_screaming_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_uppercase() || c == '_' || c.is_numeric())
}

/// Main handler for the provide! macro
pub fn handle_provide(input: TokenStream) -> Result<TokenStream> {
    let ProvideInput { token, variant } = syn::parse2(input)?;

    // Convert token to TokenStream
    let token_ts = token_type_to_tokens(&token);

    match variant {
        // Value provider
        ProviderVariant::Value(value_expr) => {
            let reconstructed = quote! { #token_ts, #value_expr };
            crate::provider_variants::handle_provider_value(reconstructed)
        }

        // Factory provider
        ProviderVariant::Factory(factory_expr) => {
            let reconstructed = quote! { #token_ts, #factory_expr };
            crate::provider_variants::handle_provider_factory(reconstructed)
        }

        // Alias provider
        ProviderVariant::Alias(existing_token) => {
            let existing_ts = token_type_to_tokens(&existing_token);
            let reconstructed = quote! { #token_ts, #existing_ts };
            crate::provider_variants::handle_provider_alias(reconstructed)
        }

        // Token provider
        ProviderVariant::TokenProvider(provider_type) => {
            let reconstructed = quote! { #token_ts, #provider_type };
            crate::provider_variants::handle_provider_token(reconstructed)
        }
    }
}

/// Convert TokenType back to a TokenStream that can be parsed by the handler macros
fn token_type_to_tokens(token: &TokenType) -> TokenStream {
    match token {
        TokenType::String(s) => {
            quote! { #s }
        }
        TokenType::Type(path) => {
            quote! { #path }
        }
        TokenType::Const(path) => {
            quote! { #path }
        }
    }
}
