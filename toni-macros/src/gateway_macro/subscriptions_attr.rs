//! `#[subscriptions]` — the impl-side message router for a gateway.
//!
//! Pairs with `#[websocket_gateway("/p")]` on the struct. Scans the impl for `#[subscribe_message]`
//! handlers and the gateway- and handler-level enhancer attrs, and emits inherent `__toni_ws_*` fns
//! that out-rank the `WsHandlersBridge` defaults at the concrete-type call sites in the generated
//! `GatewayTrait` impl. It owns only the *aggregate* — the `handle_event` match over the variable set
//! of handlers, plus the enhancers descriptor. Single-slot connection hooks (`#[on_connect]` /
//! `#[on_disconnect]` / `#[after_init]`) are their own per-method macros, so they are left intact here
//! and a gateway with only hooks needs no `#[subscriptions]` at all.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, ImplItem, ItemImpl, LitStr, Result, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute,
};
use crate::shared::attr_is;

pub fn handle_subscriptions(item: TokenStream) -> Result<TokenStream> {
    let impl_block = parse2::<ItemImpl>(item)?;
    let struct_name = crate::utils::extracts::extract_impl_self_ident(&impl_block)?;

    let mut message_handlers = Vec::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item {
            if let Some(event_name) = extract_subscribe_message_event(&method.attrs) {
                message_handlers.push((event_name, method.clone()));
            }
        }
    }

    // User errors are preserved past the macro boundary as `ExecutionResult::Err` carrying the
    // transport's `WsError`. `Into::into` calls the `From<E: Error> for WsError` blanket so domain
    // errors implementing `toni::Error` lift automatically.
    let match_arms: Vec<_> = message_handlers
        .iter()
        .map(|(event, method)| {
            let method_name = &method.sig.ident;
            let (extractions, call_args) = handler_params(method);
            quote! {
                #event => {
                    #(#extractions)*
                    match self.#method_name(#(#call_args),*).await {
                        Ok(__output) => ::toni::http_helpers::ExecutionResult::Ok(__output),
                        Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                            ::std::convert::Into::<::toni::WsError>::into(__err),
                        ),
                    }
                }
            }
        })
        .collect();

    let enhancers_impl = build_enhancers_fn(&impl_block, &message_handlers)?;

    // Re-emit the impl with the consumed `#[subscribe_message]` and enhancer attrs stripped.
    // `#[new]`, the `#[on_*]` lifecycle attrs, and the `#[on_connect]`/`#[on_disconnect]`/
    // `#[after_init]` connection-hook attrs are LEFT intact so their own macros expand into the
    // `__toni_ctor_*` / `__toni_lc_*` / `__toni_ws_*` bridges.
    let mut impl_def = impl_block.clone();
    impl_def.attrs.retain(|attr| !has_enhancer_attribute(attr));
    for item in impl_def.items.iter_mut() {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr_is(attr, "subscribe_message") && !has_enhancer_attribute(attr)
            });
        }
    }

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        impl #struct_name {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            async fn __toni_ws_handle_event(
                &self,
                __ctx: &mut ::toni::context::WsContext,
            ) -> ::toni::http_helpers::ExecutionResult<::toni::WsHandlerOutput, ::toni::WsError> {
                let __event = ::std::string::String::from(__ctx.event());
                match __event.as_str() {
                    #(#match_arms)*
                    _ => ::toni::http_helpers::ExecutionResult::Err(
                        ::toni::WsError::EventNotFound(format!("Unknown event: {}", __event)),
                    ),
                }
            }

            #enhancers_impl
        }
    })
}

/// Extraction for a handler's parameters, in signature order.
///
/// Every parameter is a `FromContext<WsContext>`, so a handler takes what it
/// needs and nothing more — the fixed `(WsClient, WsMessage)` pair is now just
/// the most common choice rather than the only one. `&mut WsContext` is passed
/// straight through, reborrowed at the call so it holds no borrow across the
/// extractions before it.
fn handler_params(method: &syn::ImplItemFn) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut extractions = Vec::new();
    let mut call_args = Vec::new();

    for input in method.sig.inputs.iter() {
        let syn::FnArg::Typed(pat_type) = input else {
            continue;
        };
        // The binding takes the whole extracted value — a pattern like
        // `Payload(order)` destructures it in the user's own signature.
        let Some(name) =
            crate::controller_macro::extractor_params::extract_param_name(&pat_type.pat)
        else {
            continue;
        };
        let ty = &*pat_type.ty;

        if is_ws_context_ref(ty) {
            call_args.push(quote! { &mut *__ctx });
            continue;
        }

        extractions.push(quote! {
            let #name = match <#ty as ::toni::extractors::FromContext<
                ::toni::context::WsContext,
            >>::extract(__ctx).await {
                ::std::result::Result::Ok(__value) => __value,
                ::std::result::Result::Err(__e) => {
                    return ::toni::http_helpers::ExecutionResult::Err(
                        ::toni::WsError::Internal(__e.to_string()),
                    );
                }
            };
        });
        call_args.push(quote! { #name });
    }

    (extractions, call_args)
}

/// `&mut WsContext` — the context itself, not something extracted from it.
fn is_ws_context_ref(ty: &syn::Type) -> bool {
    let syn::Type::Reference(type_ref) = ty else {
        return false;
    };
    matches!(&*type_ref.elem, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "WsContext"))
}

/// Collect gateway-level and per-handler enhancer tokens into the `__toni_ws_enhancers` inherent fn,
/// which shadows the bridge default and builds the `GatewayEnhancers` descriptor the resolver reads.
fn build_enhancers_fn(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let gateway_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let enhancer_infos = create_enhancer_infos(gateway_enhancers_attr, Vec::new())?;

    let tokens_for = |key: &str| -> Vec<TokenStream> {
        let empty = Vec::new();
        enhancer_infos
            .get(key)
            .unwrap_or(&empty)
            .iter()
            .filter(|info| !info.token_expr.is_empty())
            .map(|info| info.token_expr.clone())
            .collect()
    };
    let guard_tokens = tokens_for("guards");
    let interceptor_tokens = tokens_for("interceptors");
    let pipe_tokens = tokens_for("pipes");
    let error_handler_tokens = tokens_for("error_handlers");

    let mut handler_entries: Vec<TokenStream> = Vec::new();
    for (event, method) in message_handlers {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let handler_infos = create_enhancer_infos(method_enhancers_attr, Vec::new())?;
        let htokens_for = |key: &str| -> Vec<TokenStream> {
            let empty = Vec::new();
            handler_infos
                .get(key)
                .unwrap_or(&empty)
                .iter()
                .filter(|info| !info.token_expr.is_empty())
                .map(|info| info.token_expr.clone())
                .collect()
        };
        let hg = htokens_for("guards");
        let hi = htokens_for("interceptors");
        let hp = htokens_for("pipes");
        let he = htokens_for("error_handlers");
        if hg.is_empty() && hi.is_empty() && hp.is_empty() && he.is_empty() {
            continue;
        }
        handler_entries.push(quote! {
            ::toni::GatewayHandlerEnhancers {
                event: #event.to_string(),
                guard_tokens: vec![#(#hg),*],
                interceptor_tokens: vec![#(#hi),*],
                pipe_tokens: vec![#(#hp),*],
                error_handler_tokens: vec![#(#he),*],
            }
        });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __toni_ws_enhancers(&self) -> ::toni::GatewayEnhancers {
            ::toni::GatewayEnhancers {
                guard_tokens: vec![#(#guard_tokens),*],
                interceptor_tokens: vec![#(#interceptor_tokens),*],
                pipe_tokens: vec![#(#pipe_tokens),*],
                error_handler_tokens: vec![#(#error_handler_tokens),*],
                handlers: vec![#(#handler_entries),*],
            }
        }
    })
}

fn extract_subscribe_message_event(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr_is(attr, "subscribe_message") {
            if let Ok(lit) = attr.parse_args::<LitStr>() {
                return Some(lit.value());
            }
        }
    }
    None
}
