//! `#[new]` — marks the DI constructor of a `#[injectable]` provider.
//!
//! Lives on the constructor method. The struct macro generates the factory from the struct's fields
//! and cannot see this method, so `#[new]` emits next to it two inherent fns named after the
//! method, one returning the constructor's dependency tokens and one resolving them and calling it,
//! and one inherent const, `__ULO_ONE_NEW_PER_TYPE`, holding both as a `ulo::__construct::Ctor`.
//! The const out-ranks the blanket `CtorBridge` default, so the struct macro's factory, which
//! always reads it, dispatches to the constructor when present and to field injection otherwise.
//!
//! The const is the one item whose name does not vary with the method, so two `#[new]` methods on
//! one type collide on it alone, as one duplicate-definition error at the two attributes.
//!
//! The method and the three emitted items are associated items, so they're legal in the impl-block
//! position a method attribute macro emits into. This lets a dependency be a constructor parameter
//! without also being a stored field.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
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
    let tokens_fn = format_ident!("__ulo_ctor_tokens_{}", method_name);
    let build_fn = format_ident!("__ulo_ctor_build_{}", method_name);
    let params = extract_params(&method)?;

    let dep_tokens: Vec<&TokenStream> = params.iter().map(|(_, _, tok)| tok).collect();
    let resolutions = params
        .iter()
        .map(|(name, ty, tok)| resolve_param(name, ty, tok));
    let arg_names: Vec<&Ident> = params.iter().map(|(name, _, _)| name).collect();

    // `#[inject]` on a parameter is read above to pick the lookup token; it is not a real attribute,
    // so it must be stripped before the method is re-emitted or rustc rejects it.
    let mut emitted_method = method.clone();
    for input in &mut emitted_method.sig.inputs {
        if let FnArg::Typed(pat_type) = input {
            pat_type
                .attrs
                .retain(|attr| !crate::shared::attr_is(attr, "inject"));
        }
    }

    Ok(quote! {
        #emitted_method

        #[doc(hidden)]
        const __ULO_ONE_NEW_PER_TYPE: ::std::option::Option<::ulo::__construct::Ctor<Self>> =
            ::std::option::Option::Some(::ulo::__construct::Ctor {
                tokens: Self::#tokens_fn,
                build: Self::#build_fn,
            });

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn #tokens_fn() -> ::std::vec::Vec<::std::string::String> {
            ::std::vec![#(#dep_tokens),*]
        }

        #[doc(hidden)]
        #[allow(unused_variables, non_snake_case)]
        fn #build_fn<'a>(
            deps: &'a ::ulo::__construct::ResolvedDeps,
            __exec_ctx: ::ulo::di::Execution,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Self> + Send + 'a>> {
            ::std::boxed::Box::pin(async move {
                #(#resolutions)*
                Self::#method_name(#(#arg_names),*)
            })
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

/// The key `#[inject(K)]` on a parameter names; `None` for bare `#[inject]` or none, where the
/// caller keys by the parameter's type.
fn extract_param_inject_token(pat_type: &syn::PatType) -> Result<Option<TokenStream>> {
    for attr in &pat_type.attrs {
        if crate::shared::attr_is(attr, "inject") {
            return crate::shared::inject_key::inject_key(attr, &pat_type.ty);
        }
    }
    Ok(None)
}

/// Resolve one constructor parameter from the dependency map, scope-aware: an execution-scoped
/// parameter is resolved in the active execution (threaded via `__exec_ctx`),
/// anything else with `Execution::None` — mirroring the field-injection
/// paths. Panics with a clear message on a missing dep or absent request context.
fn resolve_param(name: &Ident, ty: &Type, token: &TokenStream) -> TokenStream {
    // A collection answers its items erased; the path below downcasts to the parameter's own type.
    if let Some(inner_trait) = crate::utils::extracts::extract_vec_arc_dyn_inner(ty) {
        return super::instance_injection::collection_field_resolution(
            name,
            ty,
            &inner_trait,
            token,
            quote! { deps },
            quote! {
                if matches!(__provider.scope(), ::ulo::di::ProviderScope::Execution) {
                    __exec_ctx.clone()
                } else {
                    ::ulo::di::Execution::None
                }
            },
        );
    }
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
            let __ctx = if matches!(__provider.scope(), ::ulo::di::ProviderScope::Execution) {
                __exec_ctx.clone()
            } else {
                ::ulo::di::Execution::None
            };
            let __any = __provider
                .resolve(__ctx)
                .await;
            *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                "Failed to downcast '{}' to {} for #[new] parameter '{}'",
                __lookup_token, stringify!(#ty), #name_str
            ))
        };
    }
}
