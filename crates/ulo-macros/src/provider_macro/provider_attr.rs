//! `#[injectable]` — the attribute form of a field-injection provider.
//!
//! Placed directly on a struct, it registers the struct as a DI provider: `#[inject]` fields are
//! dependencies, `#[default(expr)]` fields are owned state, and `#[injectable(scope = "…")]` sets the
//! scope (default singleton).
//!
//! Construction logic and lifecycle hooks live on the struct's `impl` via `#[new]` / `#[on_module_init]`
//! and friends, exactly as with the struct alone.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    Expr, ExprLit, Ident, ItemStruct, Lit, MetaNameValue, Result, Token, parse::Parser as _,
    parse2, punctuated::Punctuated,
};

use crate::shared::scope_parser::ProviderScope;

use super::instance_injection::{add_inject_fields, generate_provider_from_struct};
use super::lifecycle_attr::Hook;

pub fn handle_provider(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;
    let (scope, scope_span) = parse_args(attr)?;

    let emitted_struct = add_inject_fields(&struct_def);
    let wiring = generate_provider_from_struct(&struct_def, scope)?;
    let hook_checks = hooks_that_cannot_fire(&struct_def.ident, scope, scope_span);

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct
        #wiring
        #hook_checks
    })
}

/// One `const` item per lifecycle hook, failing const evaluation when the type carries that hook
/// and its scope never fires one. The framework keeps no execution-scoped or transient instance,
/// so it has none to run a startup or shutdown hook on. The hook sits on a method the struct macro
/// cannot see, so each item reads the flag the hook's macro emits over the blanket default in
/// `ulo::__lifecycle::LifecycleFlags`, spanned at the scope that refuses it.
fn hooks_that_cannot_fire(name: &Ident, scope: ProviderScope, span: Span) -> TokenStream {
    let why = match scope {
        ProviderScope::Singleton => return TokenStream::new(),
        ProviderScope::Execution => "is execution-scoped, built per execution and dropped with it",
        ProviderScope::Transient => "is transient, built afresh for every consumer",
    };
    Hook::ALL
        .iter()
        .map(|hook| {
            let flag = format_ident!("{}", hook.flag());
            let message = format!(
                "`#[{}]` on `{name}` never fires: `{name}` {why}, so the framework holds no \
                 instance of it to run the hook on; make `{name}` a singleton to keep the hook, \
                 or remove it",
                hook.attr_name()
            );
            quote_spanned! {span=>
                const _: () = {
                    #[allow(unused_imports)]
                    use ::ulo::__lifecycle::LifecycleFlags as _;
                    if <#name>::#flag {
                        ::core::panic!(#message);
                    }
                };
            }
        })
        .collect()
}

/// Parse `scope = "…"` from the attribute arguments (`#[injectable(scope = "…")]`), with the span
/// of the scope's value for a diagnostic about it.
fn parse_args(attr: TokenStream) -> Result<(ProviderScope, Span)> {
    let mut scope = ProviderScope::default();
    let mut span = Span::call_site();

    if attr.is_empty() {
        return Ok((scope, span));
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
                span = syn::spanned::Spanned::span(&nv.value);
                scope = match value.as_str() {
                    "singleton" => ProviderScope::Singleton,
                    "execution" => ProviderScope::Execution,
                    "request" => {
                        return Err(syn::Error::new_spanned(
                            &nv.value,
                            crate::shared::scope_parser::SCOPE_RENAMED,
                        ));
                    }
                    "transient" => ProviderScope::Transient,
                    other => {
                        return Err(syn::Error::new_spanned(
                            &nv.value,
                            format!(
                                "Invalid scope: '{}'. Must be 'singleton', 'execution', or 'transient'",
                                other
                            ),
                        ));
                    }
                };
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &nv.path,
                    format!("Unknown #[injectable] key: '{}'. Expected 'scope'", other),
                ));
            }
        }
    }

    Ok((scope, span))
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
            "expected a string literal, e.g. scope = \"execution\"",
        ))
    }
}
