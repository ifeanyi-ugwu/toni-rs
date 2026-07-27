use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, Result, Token, punctuated::Punctuated, spanned::Spanned};

fn is_enhancer(segment: &Ident) -> bool {
    matches!(
        segment.to_string().as_str(),
        "use_guards" | "use_interceptors" | "use_pipes" | "use_error_handlers"
    )
}

/// Matches by the path's last segment so path-qualified forms
/// (`#[toni::use_guards(…)]`) are recognized alongside the bare ones.
pub fn has_enhancer_attribute(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| is_enhancer(&segment.ident))
}

/// Represents an enhancer that can be resolved from DI or directly instantiated
#[derive(Clone)]
pub struct EnhancerInfo {
    /// The type identifier of the enhancer (for token-based DI resolution)
    pub type_ident: Ident,
    /// The token used for DI resolution
    pub token_expr: TokenStream,
    /// The full instantiation expression (for direct instantiation fallback)
    /// E.g., `MyGuard` or `MyGuard::new()` or `MyGuard::new("admin")`
    pub instance_expr: TokenStream,
}

/// Create enhancer infos from attributes for DI resolution
/// Returns a map of enhancer type -> list of EnhancerInfo
pub fn create_enhancer_infos(
    controller_enhancers_attr: Vec<(&Ident, &Attribute)>,
    method_enhancers_attr: Vec<(&Ident, &Attribute)>,
) -> Result<HashMap<String, Vec<EnhancerInfo>>> {
    let mut enhancers: HashMap<String, Vec<EnhancerInfo>> = HashMap::new();

    // Controller-level first; method-level appends to the same key, it does not replace.
    for (ident, attr) in controller_enhancers_attr
        .into_iter()
        .chain(method_enhancers_attr)
    {
        // Parse as expressions to support both `MyGuard` and `MyGuard::new()`
        let arg_exprs = attr
            .parse_args_with(Punctuated::<syn::Expr, Token![,]>::parse_terminated)
            .map_err(|_| Error::new(attr.span(), "Invalid attribute format"))?;

        // Normalize attribute names: strip "use_" prefix
        let key = ident.to_string().replace("use_", "");

        for arg_expr in arg_exprs {
            // Extract the type identifier and optionally the instance expression
            let (type_ident, instance_expr_opt) = extract_enhancer_info(&arg_expr)?;

            // Generate token based on the type of enhancer
            let (token_expr, instance_expr) = if let Some(expr) = instance_expr_opt {
                // Check if this is a string token (dummy ident __StringToken)
                if type_ident == "__StringToken" {
                    // String literal: use the expression as token
                    (expr, quote! {})
                } else {
                    // Direct instantiation: no token, use expression for instance
                    (quote! {}, expr)
                }
            } else {
                // Type-name syntax: generate type token
                (
                    quote! { std::any::type_name::<#type_ident>().to_string() },
                    quote! {},
                )
            };

            let info = EnhancerInfo {
                type_ident,
                token_expr,
                instance_expr,
            };

            enhancers.entry(key.clone()).or_default().push(info);
        }
    }

    Ok(enhancers)
}

/// Extract enhancer information from an expression
/// Returns: (type_ident, optional_instance_expr)
///
/// Supports:
/// - `MyGuard` → (`MyGuard`, None) - DI resolution only (generates type token)
/// - `"AUTH_GUARD"` → (`__StringToken`, None) - DI resolution with string token
/// - `APP_GUARD` → (`__ConstToken`, None) - DI resolution with const token
/// - `MyGuard{}` → (`MyGuard`, Some(`MyGuard`)) - Direct instantiation (generates instance)
/// - `MyGuard::new()` → (`MyGuard`, Some(`MyGuard::new()`)) - Direct instantiation via constructor (generates instance)
/// - `MyGuard::new("admin")` → (`MyGuard`, Some(`MyGuard::new("admin")`)) - Direct instantiation with args (generates instance)
fn extract_enhancer_info(expr: &syn::Expr) -> Result<(Ident, Option<TokenStream>)> {
    match expr {
        // String literal: "AUTH_GUARD"
        // Generates: token from string → DI resolution
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit_str),
            ..
        }) => {
            let token_string = lit_str.clone();
            // Create a dummy ident for tracking
            let type_ident = Ident::new("__StringToken", lit_str.span());
            // Generate token expression that returns the string
            let token_expr = quote! { #token_string.to_string() };
            // Return as "instance_expr" to override token generation
            Ok((type_ident, Some(token_expr)))
        }
        // Simple path (just type name): MyGuard or APP_GUARD (const)
        // Generates: token only → DI resolution required
        syn::Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
            let type_ident = expr_path.path.segments[0].ident.clone();
            Ok((type_ident, None))
        }
        // Struct instantiation: MyGuard{} or MyGuard { field: value }
        // Generates: instance expression → direct instantiation
        syn::Expr::Struct(expr_struct) => {
            if let Some(first_segment) = expr_struct.path.segments.first() {
                let type_ident = first_segment.ident.clone();
                let instance_expr = quote! { #expr };
                return Ok((type_ident, Some(instance_expr)));
            }
            Err(Error::new(
                expr.span(),
                "Expected valid struct path in struct expression",
            ))
        }
        // Constructor call: MyGuard::new() or MyGuard::new("args")
        // Generates: instance expression → direct instantiation
        syn::Expr::Call(expr_call) => {
            if let syn::Expr::Path(path_expr) = &*expr_call.func {
                // Get the first segment (the type name before ::)
                if let Some(first_segment) = path_expr.path.segments.first() {
                    let type_ident = first_segment.ident.clone();
                    let instance_expr = quote! { #expr };
                    return Ok((type_ident, Some(instance_expr)));
                }
            }
            Err(Error::new(
                expr.span(),
                "Expected type identifier or Type::new() expression",
            ))
        }
        _ => Err(Error::new(
            expr.span(),
            "Expected type identifier (MyGuard), string literal (\"AUTH_GUARD\"), struct literal (MyGuard{}), or constructor call (MyGuard::new())",
        )),
    }
}

/// Collect enhancer attributes as (name, attribute) pairs in declaration order.
///
/// The name is the path's last segment, so `#[use_guards(…)]` and `#[toni::use_guards(…)]`
/// collect identically. Stacked attributes of the same kind each get their own pair;
/// [`create_enhancer_infos`] appends them in order.
pub fn get_enhancers_attr(attrs: &[Attribute]) -> Result<Vec<(&Ident, &Attribute)>> {
    Ok(attrs
        .iter()
        .filter_map(|attr| {
            let segment = attr.path().segments.last()?;
            is_enhancer(&segment.ident).then_some((&segment.ident, attr))
        })
        .collect())
}
