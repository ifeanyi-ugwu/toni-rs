//! `#[subscriptions]` — the impl-side message router for a gateway.
//!
//! Pairs with `#[websocket_gateway("/p")]` on the struct. Scans the impl for `#[subscribe_message]`
//! handlers and the gateway- and handler-level enhancer attrs, and emits inherent `__ulo_ws_*` fns
//! that out-rank the `WsHandlersBridge` defaults at the concrete-type call sites in the generated
//! `Gateway` impl. It owns only the *aggregate* — the `handle_event` match over the variable set
//! of handlers, plus the enhancers descriptor. Single-slot connection hooks (`#[on_connect]` /
//! `#[on_disconnect]` / `#[after_init]`) are their own per-method macros, so they are left intact here
//! and a gateway with only hooks needs no `#[subscriptions]` at all.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, ImplItem, ItemImpl, LitStr, Result, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, declares_anything, enhancer_entries, get_enhancers_attr,
    has_enhancer_attribute,
};
use crate::shared::attr_is;
use crate::shared::set_metadata::{get_metadata_exprs, merged_metadata_exprs, metadata_ctor};

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
    // errors implementing `ulo::Error` lift automatically.
    let match_arms: Vec<_> = message_handlers
        .iter()
        .map(|(event, method)| {
            let method_name = &method.sig.ident;
            let (extractions, call_args) = handler_params(method);
            quote! {
                #event => {
                    #(#extractions)*
                    match ::ulo::dispatch::IntoOutput::<::ulo::__enhancer::Ws>::into_output(
                        self.#method_name(#(#call_args),*).await,
                    ) {
                        Ok(__output) => ::ulo::dispatch::ExecutionResult::Ok(__output),
                        Err(__err) => ::ulo::dispatch::ExecutionResult::Err(__err),
                    }
                }
            }
        })
        .collect();

    let enhancers_impl = build_enhancers_fn(&impl_block, &message_handlers)?;
    let metadata_fns = build_metadata_fns(&impl_block, &message_handlers)?;

    // Re-emit the impl with the consumed `#[subscribe_message]` and enhancer attrs stripped.
    // `#[new]`, the `#[on_*]` lifecycle attrs, and the `#[on_connect]`/`#[on_disconnect]`/
    // `#[after_init]` connection-hook attrs are LEFT intact so their own macros expand into the
    // `__ULO_ONE_NEW_PER_TYPE` / `__ulo_lc_*` / `__ulo_ws_*` bridges.
    let mut impl_def = impl_block.clone();
    impl_def.attrs.retain(|attr| !has_enhancer_attribute(attr));
    for item in impl_def.items.iter_mut() {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr_is(attr, "subscribe_message")
                    && !attr_is(attr, "set_metadata")
                    && !has_enhancer_attribute(attr)
            });
        }
    }

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        impl #struct_name {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            async fn __ulo_ws_handle_event(
                &self,
                __ctx: &::ulo::ws::WsContext,
            ) -> ::ulo::dispatch::ExecutionResult<::ulo::ws::WsHandlerOutput, ::ulo::ws::WsError> {
                let __event = ::std::string::String::from(__ctx.event());
                match __event.as_str() {
                    #(#match_arms)*
                    // A typed event, so a `#[catch(Unrouted)]` handler can claim
                    // it. Unclaimed it renders the `NotFound` envelope it always
                    // did.
                    _ => ::ulo::dispatch::ExecutionResult::Err(
                        ::ulo::ws::WsError::AppError(::std::sync::Arc::new(
                            ::ulo::errors::Unrouted::new(__event),
                        )),
                    ),
                }
            }

            #enhancers_impl
            #metadata_fns
        }
    })
}

/// The gateway's declared metadata: the impl block's entries as the base, and one merged map per
/// event whose handler adds to them. An event the handler does not annotate reads the base.
fn build_metadata_fns(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let base = metadata_ctor(&get_metadata_exprs(&impl_block.attrs)?)
        .unwrap_or_else(|| quote! { ::ulo::context::Metadata::new() });

    let mut entries: Vec<TokenStream> = Vec::new();
    for (event, method) in message_handlers {
        if get_metadata_exprs(&method.attrs)?.is_empty() {
            continue;
        }
        let merged = merged_metadata_exprs(&impl_block.attrs, &method.attrs)?;
        let ctor = metadata_ctor(&merged).expect("non-empty");
        entries.push(quote! { (#event.to_string(), #ctor) });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __ulo_ws_metadata() -> ::ulo::context::Metadata {
            #base
        }

        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __ulo_ws_handler_metadata() -> Vec<(String, ::ulo::context::Metadata)> {
            vec![#(#entries),*]
        }
    })
}

/// Extraction for a handler's parameters, in signature order.
///
/// Every parameter is a `FromContext<WsContext>`, so a handler takes what it
/// needs and nothing more — the fixed `(WsClient, WsMessage)` pair is now just
/// the most common choice rather than the only one. `&WsContext` is passed
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
            call_args.push(quote! { &*__ctx });
            continue;
        }

        extractions.push(quote! {
            let #name = match <#ty as ::ulo::extract::FromContext<
                ::ulo::ws::WsContext,
            >>::extract(__ctx).await {
                ::std::result::Result::Ok(__value) => __value,
                ::std::result::Result::Err(__e) => {
                    return ::ulo::dispatch::ExecutionResult::Err(
                        ::ulo::ws::WsError::Internal(__e.to_string()),
                    );
                }
            };
        });
        call_args.push(quote! { #name });
    }

    (extractions, call_args)
}

/// `&WsContext` — the context itself, not something extracted from it.
fn is_ws_context_ref(ty: &syn::Type) -> bool {
    let syn::Type::Reference(type_ref) = ty else {
        return false;
    };
    matches!(&*type_ref.elem, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "WsContext"))
}

/// Collect gateway-level and per-handler enhancer tokens into the `__ulo_ws_enhancers` inherent fn,
/// which shadows the bridge default and builds the `GatewayEnhancers` descriptor the resolver reads.
fn build_enhancers_fn(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let transport = quote! { ::ulo::dispatch::Ws };
    let gateway_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let enhancer_infos = create_enhancer_infos(gateway_enhancers_attr, Vec::new())?;
    let guards = enhancer_entries(&enhancer_infos, "guards", &transport);
    let interceptors = enhancer_entries(&enhancer_infos, "interceptors", &transport);
    let error_handlers = enhancer_entries(&enhancer_infos, "error_handlers", &transport);

    let mut handler_entries: Vec<TokenStream> = Vec::new();
    for (event, method) in message_handlers {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let handler_infos = create_enhancer_infos(method_enhancers_attr, Vec::new())?;
        if !declares_anything(&handler_infos) {
            continue;
        }
        let hg = enhancer_entries(&handler_infos, "guards", &transport);
        let hi = enhancer_entries(&handler_infos, "interceptors", &transport);
        let he = enhancer_entries(&handler_infos, "error_handlers", &transport);
        handler_entries.push(quote! {
            ::ulo::ws::GatewayHandlerEnhancers {
                event: #event.to_string(),
                guards: vec![#(#hg),*],
                interceptors: vec![#(#hi),*],
                error_handlers: vec![#(#he),*],
            }
        });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __ulo_ws_enhancers(&self) -> ::ulo::ws::GatewayEnhancers {
            ::ulo::ws::GatewayEnhancers {
                guards: vec![#(#guards),*],
                interceptors: vec![#(#interceptors),*],
                error_handlers: vec![#(#error_handlers),*],
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
