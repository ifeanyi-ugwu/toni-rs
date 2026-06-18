//! `#[injectable]` — the attribute form of a field-injection provider.
//!
//! Placed directly on a struct, it registers the struct as a DI provider: `#[inject]` fields are
//! dependencies, `#[default(expr)]` fields are owned state, and `#[injectable(scope = "…")]` sets the
//! scope (default singleton). Unlike a derive, an attribute macro re-emits the struct, so it adds
//! the `Clone` impl the provider wrapper needs when the user hasn't — the struct declaration carries
//! no `#[derive(Clone, …)]` ceremony.
//!
//! Construction logic and lifecycle hooks live on the struct's `impl` via `#[new]` / `#[on_module_init]`
//! and friends, exactly as with the struct alone.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, ExprLit, Ident, ItemStruct, Lit, MetaNameValue, Result, Token, parse::Parser as _,
    parse2, punctuated::Punctuated,
};

use crate::shared::scope_parser::ProviderScope;

use super::instance_injection::{add_clone_and_inject_fields, generate_provider_from_struct};

pub fn handle_provider(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;
    let (scope, init) = parse_args(attr)?;

    // Re-emit the struct with Clone (if absent) + InjectFields so `#[inject]`/`#[default]` stay
    // valid; then emit the provider wiring beside it.
    let emitted_struct = add_clone_and_inject_fields(&struct_def);
    let wiring = generate_provider_from_struct(&struct_def, scope, init)?;

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct
        #wiring
    })
}

/// Parse `scope = "…"` and `init = "…"` from the attribute arguments (`#[injectable(scope = "…")]`).
fn parse_args(attr: TokenStream) -> Result<(ProviderScope, Option<String>)> {
    let mut scope = ProviderScope::default();
    let mut init: Option<String> = None;

    if attr.is_empty() {
        return Ok((scope, init));
    }

    let pairs = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(attr)?;
    for nv in pairs {
        let key = nv
            .path
            .get_ident()
            .map(Ident::to_string)
            .unwrap_or_default();
        let value = str_lit_value(&nv.value)?;
        match key.as_str() {
            "scope" => {
                scope = match value.as_str() {
                    "singleton" => ProviderScope::Singleton,
                    "request" => ProviderScope::Request,
                    "transient" => ProviderScope::Transient,
                    other => {
                        return Err(syn::Error::new_spanned(
                            &nv.value,
                            format!(
                                "Invalid scope: '{}'. Must be 'singleton', 'request', or 'transient'",
                                other
                            ),
                        ));
                    }
                };
            }
            "init" => init = Some(value),
            other => {
                return Err(syn::Error::new_spanned(
                    &nv.path,
                    format!(
                        "Unknown #[injectable] key: '{}'. Expected 'scope' or 'init'",
                        other
                    ),
                ));
            }
        }
    }

    Ok((scope, init))
}

fn str_lit_value(expr: &Expr) -> Result<String> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(syn::Error::new_spanned(
            expr,
            "expected a string literal, e.g. scope = \"request\"",
        ))
    }
}
