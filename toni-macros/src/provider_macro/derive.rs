//! `#[derive(Injectable)]` — field-injection provider registration.
//!
//! A derive sees only the struct, never an impl block. That is exactly the right shape for
//! field injection: dependencies are declared as `#[inject]` fields, owned state as `#[default]`
//! fields, and the user's own `impl` stays untouched. Scope and an optional constructor name
//! come from a companion `#[provider(scope = "request", init = "new")]` attribute.

use proc_macro2::TokenStream;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, ItemStruct, Lit, MetaNameValue, Result, Token,
    parse2, punctuated::Punctuated,
};

use crate::shared::scope_parser::ProviderScope;

use super::instance_injection::generate_provider_from_struct;

pub fn handle_derive_injectable(input: TokenStream) -> Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;

    let Data::Struct(data_struct) = derive_input.data else {
        return Err(syn::Error::new_spanned(
            &derive_input.ident,
            "Injectable can only be derived on structs. Register providers for other types \
             with `provide!` / `provider_factory!`.",
        ));
    };

    if !derive_input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &derive_input.generics,
            "#[derive(Injectable)] does not support generic structs yet. Register a concrete \
             instance with `provider_factory!`, or open a follow-up if you need this.",
        ));
    }

    let struct_def = ItemStruct {
        attrs: derive_input.attrs,
        vis: derive_input.vis,
        struct_token: data_struct.struct_token,
        ident: derive_input.ident,
        generics: derive_input.generics,
        fields: data_struct.fields,
        semi_token: data_struct.semi_token,
    };

    let (scope, init) = parse_provider_attr(&struct_def.attrs)?;
    generate_provider_from_struct(&struct_def, scope, init)
}

/// Parse the companion `#[provider(scope = "…", init = "…")]` attribute. Both keys are
/// optional; scope defaults to singleton. `init` is carried through so the generator can
/// reject it with a precise message (constructor injection needs the impl, which a derive
/// cannot see).
fn parse_provider_attr(attrs: &[Attribute]) -> Result<(ProviderScope, Option<String>)> {
    let mut scope = ProviderScope::default();
    let mut init: Option<String> = None;

    for attr in attrs {
        if !attr.path().is_ident("provider") {
            continue;
        }

        let pairs =
            attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;

        for nv in pairs {
            let key = nv
                .path
                .get_ident()
                .map(|i| i.to_string())
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
                        format!("Unknown #[provider] key: '{}'. Expected 'scope' or 'init'", other),
                    ));
                }
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
