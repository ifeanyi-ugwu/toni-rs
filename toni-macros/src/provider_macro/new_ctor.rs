//! `#[new]` — marks the DI constructor of a `#[provider]` provider.
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

    // `#[inject]` on a parameter is read above to pick the lookup token; it is not a real attribute,
    // so it must be stripped before the method is re-emitted or rustc rejects it.
    let mut emitted_method = method.clone();
    for input in &mut emitted_method.sig.inputs {
        if let FnArg::Typed(pat_type) = input {
            pat_type.attrs.retain(|attr| !attr.path().is_ident("inject"));
        }
    }

    Ok(quote! {
        #emitted_method

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn __toni_ctor_tokens() -> ::std::option::Option<::std::vec::Vec<::std::string::String>> {
            ::std::option::Option::Some(::std::vec![#(#dep_tokens),*])
        }

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn __toni_ctor_build<'a>(
            deps: &'a ::toni::__construct::ResolvedDeps,
            request_parts: ::std::option::Option<&'a ::toni::http_helpers::RequestPart>,
        ) -> ::std::option::Option<
            ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Self> + Send + 'a>>
        > {
            ::std::option::Option::Some(::std::boxed::Box::pin(async move {
                let __request_cache = ::toni::traits_helpers::RequestCache::new();
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

/// Resolve one constructor parameter from the dependency map, scope-aware: a request-scoped
/// parameter is resolved with the active HTTP context (threaded via `request_parts` +
/// `__request_cache`), anything else with `ProviderContext::None` — mirroring the field-injection
/// paths. Panics with a clear message on a missing dep or absent request context.
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
            let __ctx = if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                ::toni::ProviderContext::Http(::toni::traits_helpers::HttpProviderContext {
                    parts: request_parts.unwrap_or_else(|| panic!(
                        "#[new] parameter '{}' is request-scoped but no HTTP request context is \
                         available; request-scoped dependencies can only be constructed within a \
                         request",
                        #name_str
                    )),
                    cache: &__request_cache,
                })
            } else {
                ::toni::ProviderContext::None
            };
            let __any = __provider
                .execute(::std::vec::Vec::new(), __ctx)
                .await;
            *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                "Failed to downcast '{}' to {} for #[new] parameter '{}'",
                __lookup_token, stringify!(#ty), #name_str
            ))
        };
    }
}
