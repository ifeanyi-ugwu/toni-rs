//! `#[catch(T)]` — function-style declaration of an `ErrorHandler<C, R>`
//! that runs only for errors of type `T`.
//!
//! Lowers a free async function into a unit struct whose `ErrorHandler` impl
//! downcasts the inbound error to `T` (returning `None` to fall through when
//! the type doesn't match) and otherwise forwards to the user's body.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, ReturnType, Type, parse_macro_input, spanned::Spanned};

pub fn catch(attr: TokenStream, item: TokenStream) -> TokenStream {
    let target_ty = parse_macro_input!(attr as Type);
    let func = parse_macro_input!(item as ItemFn);

    match expand(target_ty, func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(target_ty: Type, func: ItemFn) -> syn::Result<TokenStream2> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.span(),
            "#[catch(T)] requires an async fn",
        ));
    }

    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            func.sig.generics.span(),
            "#[catch(T)] does not support generic functions",
        ));
    }

    let name = func.sig.ident.clone();
    let vis = func.vis.clone();
    let body = func.block.clone();

    let inputs: Vec<&FnArg> = func.sig.inputs.iter().collect();
    if inputs.len() != 2 {
        return Err(syn::Error::new(
            func.sig.inputs.span(),
            "#[catch(T)] expects exactly two arguments: (err: &T, ctx: &CtxType)",
        ));
    }

    let err_arg = expect_typed(inputs[0])?;
    let ctx_arg = expect_typed(inputs[1])?;

    let err_ident = pat_ident(&err_arg.pat)?;
    let ctx_ident = pat_ident(&ctx_arg.pat)?;

    let err_ref_ty = expect_shared_ref(&err_arg.ty, "first argument must be `&T`")?;
    let ctx_ty = expect_shared_ref(&ctx_arg.ty, "second argument must be `&CtxType`")?;

    let response_ty = match &func.sig.output {
        ReturnType::Default => {
            return Err(syn::Error::new(
                func.sig.output.span(),
                "#[catch(T)] requires an explicit response return type",
            ));
        }
        ReturnType::Type(_, ty) => ty.clone(),
    };

    let target_ref = &target_ty;
    let inner_ident = quote::format_ident!("__catch_{}_inner", name);

    let expanded = quote! {
        #[allow(non_camel_case_types)]
        #vis struct #name;

        #[doc(hidden)]
        async fn #inner_ident(#err_ident: &#err_ref_ty, #ctx_ident: &#ctx_ty) -> #response_ty {
            #body
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ErrorHandler<#ctx_ty, #response_ty> for #name {
            async fn handle_error(
                &self,
                error: ::toni::traits_helpers::ChainError<'_>,
                ctx: &#ctx_ty,
            ) -> ::std::option::Option<#response_ty> {
                let target: &#target_ref = error.downcast_ref::<#target_ref>()?;
                ::std::option::Option::Some(#inner_ident(target, ctx).await)
            }
        }
    };

    Ok(expanded)
}

fn expect_typed(arg: &FnArg) -> syn::Result<&syn::PatType> {
    match arg {
        FnArg::Typed(pt) => Ok(pt),
        FnArg::Receiver(r) => Err(syn::Error::new(
            r.span(),
            "#[catch(T)] is a free-function attribute — `self` is not allowed",
        )),
    }
}

fn pat_ident(pat: &Pat) -> syn::Result<&syn::Ident> {
    if let Pat::Ident(p) = pat {
        Ok(&p.ident)
    } else {
        Err(syn::Error::new(
            pat.span(),
            "#[catch(T)] argument patterns must be plain identifiers",
        ))
    }
}

fn expect_shared_ref<'a>(ty: &'a Type, msg: &str) -> syn::Result<&'a Type> {
    if let Type::Reference(r) = ty {
        if r.mutability.is_some() {
            return Err(syn::Error::new(
                r.span(),
                "#[catch(T)] arguments must be shared references (&T), not &mut",
            ));
        }
        Ok(&r.elem)
    } else {
        Err(syn::Error::new(ty.span(), msg))
    }
}
