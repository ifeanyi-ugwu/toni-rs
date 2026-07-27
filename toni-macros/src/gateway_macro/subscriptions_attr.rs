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
            quote! {
                #event => {
                    match self.#method_name(client, message).await {
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
                client: ::toni::WsClient,
                message: ::toni::WsMessage,
                event: &str,
            ) -> ::toni::http_helpers::ExecutionResult<::toni::WsHandlerOutput, ::toni::WsError> {
                match event {
                    #(#match_arms)*
                    _ => ::toni::http_helpers::ExecutionResult::Err(
                        ::toni::WsError::EventNotFound(format!("Unknown event: {}", event)),
                    ),
                }
            }

            #enhancers_impl
        }
    })
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
