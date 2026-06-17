//! Constructor-parameter helpers shared by the structural macros (`#[websocket_gateway]`,
//! `#[rpc_controller]`, `#[grpc_service]`) for detecting and reading a `new()`/`#[new]` constructor's
//! dependency tokens off an impl block.

use proc_macro2::TokenStream;
use syn::{FnArg, Ident, ImplItem, ItemImpl, Pat, Result, Type};

use crate::{shared::attr_is, utils::extracts::extract_type_token};

/// Check if the impl block has a `new()` method
pub fn has_new_method(impl_block: &ItemImpl) -> bool {
    impl_block.items.iter().any(|item| {
        if let ImplItem::Fn(method) = item {
            method.sig.ident == "new"
        } else {
            false
        }
    })
}

/// Extract the #[inject] or #[inject(token)] attribute from a parameter
/// Returns:
/// - None: no #[inject] attribute
/// - Some(None): #[inject] without token (use type-based token)
/// - Some(Some(token_expr)): #[inject("TOKEN")] or #[inject(Type)] with custom token
fn extract_param_inject_attr(pat_type: &syn::PatType) -> Result<Option<Option<TokenStream>>> {
    for attr in &pat_type.attrs {
        if attr_is(attr, "inject") {
            // Check if there's an argument
            if attr.meta.require_path_only().is_ok() {
                // #[inject] without arguments - use type-based token
                return Ok(Some(None));
            } else {
                // #[inject("TOKEN")] or #[inject(Type)] or #[inject(CONST)]
                // Parse as TokenType to support all token formats
                let token_type: crate::shared::TokenType = attr.parse_args()?;
                let token_expr = token_type.to_token_expr();
                return Ok(Some(Some(token_expr)));
            }
        }
    }
    Ok(None)
}

/// Extract parameters from a constructor method (init or new())
/// Supports #[inject] attribute on parameters to specify custom DI tokens
pub fn extract_constructor_params(
    impl_block: &ItemImpl,
    method_name: &str,
) -> Result<Vec<(Ident, Type, TokenStream)>> {
    // Find the method
    let method = impl_block.items.iter().find_map(|item| {
        if let ImplItem::Fn(method) = item {
            if method.sig.ident == method_name {
                Some(method)
            } else {
                None
            }
        } else {
            None
        }
    });

    let method = match method {
        Some(m) => m,
        None => return Ok(Vec::new()), // Method not found, return empty
    };

    let mut params = Vec::new();

    // Extract parameters (skip &self, &mut self, self)
    for input in &method.sig.inputs {
        match input {
            FnArg::Receiver(_) => continue, // Skip self parameters
            FnArg::Typed(pat_type) => {
                // Extract parameter name
                let param_name = match &*pat_type.pat {
                    Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                    _ => continue, // Skip complex patterns
                };

                // Extract parameter type
                let param_type = (*pat_type.ty).clone();

                // Check for #[inject] attribute on parameter
                let inject_attr = extract_param_inject_attr(pat_type)?;

                // Determine the lookup token
                let lookup_token_expr = if let Some(custom_token) = inject_attr {
                    if let Some(token_expr) = custom_token {
                        // #[inject("TOKEN")] or #[inject(Type)] - use custom token
                        token_expr
                    } else {
                        // #[inject] - use type-based token
                        extract_type_token(&param_type)?
                    }
                } else {
                    // No #[inject] attribute - use type-based token (default behavior)
                    extract_type_token(&param_type)?
                };

                params.push((param_name, param_type, lookup_token_expr));
            }
        }
    }

    Ok(params)
}
