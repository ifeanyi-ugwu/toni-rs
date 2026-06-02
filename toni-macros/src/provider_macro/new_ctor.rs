//! `#[new]` — marks the DI constructor of a `#[derive(Injectable)]` provider.
//!
//! Lives on the constructor method. The derive generates the factory from the struct's fields and
//! cannot see this method, so `#[new]` emits two inherent associated fns next to it —
//! `__toni_ctor_tokens` (the constructor's dependency tokens) and an async `__toni_ctor_build`
//! (resolve those dependencies, call the constructor). These out-rank the blanket
//! `toni::__construct::CtorBridge` defaults, so the derive's factory — which always calls
//! `Self::__toni_ctor_build(..)` — dispatches to the constructor when present and to field
//! injection otherwise.
//!
//! Both the method and the two emitted fns are associated items, so they're legal in the
//! impl-block position a method attribute macro emits into. This lets a dependency be a
//! constructor parameter without also being a stored field.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, Ident, ImplItemFn, Pat, Result, Type, parse2, spanned::Spanned};

use crate::utils::extracts::extract_type_token;

type CtorParam = (Ident, Type, TokenStream);

pub fn handle_new(item: TokenStream) -> Result<TokenStream> {
    let method: ImplItemFn = parse2(item)?;

    if !matches!(method.sig.output, syn::ReturnType::Type(..)) {
        return Err(syn::Error::new(
            method.sig.span(),
            "#[new] must annotate a constructor returning `Self` (or the concrete type)",
        ));
    }

    let method_name = method.sig.ident.clone();
    let params = extract_params(&method)?;

    let dep_tokens: Vec<&TokenStream> = params.iter().map(|(_, _, tok)| tok).collect();
    let resolutions = params.iter().map(|(name, ty, tok)| resolve_param(name, ty, tok));
    let arg_names: Vec<&Ident> = params.iter().map(|(name, _, _)| name).collect();

    Ok(quote! {
        #method

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn __toni_ctor_tokens() -> ::std::option::Option<::std::vec::Vec<::std::string::String>> {
            ::std::option::Option::Some(::std::vec![#(#dep_tokens),*])
        }

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn __toni_ctor_build<'a>(
            deps: &'a ::toni::__construct::ResolvedDeps,
        ) -> ::std::option::Option<
            ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Self> + Send + 'a>>
        > {
            ::std::option::Option::Some(::std::boxed::Box::pin(async move {
                #(#resolutions)*
                Self::#method_name(#(#arg_names),*)
            }))
        }
    })
}

fn extract_params(method: &ImplItemFn) -> Result<Vec<CtorParam>> {
    let mut params = Vec::new();
    for input in &method.sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            return Err(syn::Error::new(
                input.span(),
                "#[new] constructor cannot take `self`; it builds the instance",
            ));
        };
        let Pat::Ident(pat_ident) = &*pat_type.pat else {
            continue;
        };
        let name = pat_ident.ident.clone();
        let ty = (*pat_type.ty).clone();
        let token = match extract_param_inject_token(pat_type)? {
            Some(custom) => custom,
            None => extract_type_token(&ty)?,
        };
        params.push((name, ty, token));
    }
    Ok(params)
}

/// `#[inject]` / `#[inject("TOKEN")]` / `#[inject(Type)]` on a parameter → custom token; otherwise
/// `None` (caller falls back to the type token).
fn extract_param_inject_token(pat_type: &syn::PatType) -> Result<Option<TokenStream>> {
    for attr in &pat_type.attrs {
        if attr.path().is_ident("inject") {
            if attr.meta.require_path_only().is_ok() {
                return Ok(None);
            }
            let token_type: crate::shared::TokenType = attr.parse_args()?;
            return Ok(Some(token_type.to_token_expr()));
        }
    }
    Ok(None)
}

/// Resolve one constructor parameter from the dependency map. Uses `ProviderContext::None`, as the
/// singleton/factory dep-resolution paths do; panics with a clear message on a missing dep.
fn resolve_param(name: &Ident, ty: &Type, token: &TokenStream) -> TokenStream {
    let name_str = name.to_string();
    quote! {
        let #name: #ty = {
            let __lookup_token = #token;
            let __provider = deps
                .get(&__lookup_token)
                .unwrap_or_else(|| panic!(
                    "Missing dependency '{}' for #[new] parameter '{}'",
                    __lookup_token, #name_str
                ));
            let __any = __provider
                .execute(::std::vec::Vec::new(), ::toni::ProviderContext::None)
                .await;
            *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                "Failed to downcast '{}' to {} for #[new] parameter '{}'",
                __lookup_token, stringify!(#ty), #name_str
            ))
        };
    }
}
