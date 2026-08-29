//! `#[patterns]` — the impl-side pattern router for an RPC controller.
//!
//! Pairs with `#[controller]` on the struct, and is what makes the controller RPC: it emits the
//! `__toni_dispatch` shadow answering `Dispatch::Rpc`, the `RpcControllerTrait` impl, and the
//! source companion. It scans the impl for `#[message_pattern]` (request-response) and
//! `#[event_pattern]` (fire-and-forget) handlers and the controller- and handler-level enhancer
//! attrs, and emits inherent `__toni_rpc_*` fns that out-rank the `RpcHandlersBridge` defaults at
//! the concrete-type call sites in the `RpcControllerTrait` impl. RPC has no connection hooks, so
//! the scan is pure aggregation. It leaves `#[new]` and `#[on_*]` intact for their own macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, ImplItem, ItemImpl, LitStr, Result, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute,
};
use crate::shared::attr_is;
use crate::shared::set_metadata::{get_metadata_exprs, merged_metadata_exprs, metadata_ctor};

pub fn handle_patterns(item: TokenStream) -> Result<TokenStream> {
    let impl_block = parse2::<ItemImpl>(item)?;
    let struct_name = crate::utils::extracts::extract_impl_self_ident(&impl_block)?;

    let mut message_handlers: Vec<(String, syn::ImplItemFn)> = Vec::new();
    let mut event_handlers: Vec<(String, syn::ImplItemFn)> = Vec::new();

    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item {
            if let Some(pattern) = extract_pattern_attr(&method.attrs, "message_pattern") {
                message_handlers.push((pattern, method.clone()));
            } else if let Some(pattern) = extract_pattern_attr(&method.attrs, "event_pattern") {
                check_event_return_type(method)?;
                event_handlers.push((pattern, method.clone()));
            }
        }
    }

    let all_patterns: Vec<&str> = message_handlers
        .iter()
        .map(|(p, _)| p.as_str())
        .chain(event_handlers.iter().map(|(p, _)| p.as_str()))
        .collect();

    // User errors flow through the dispatcher as `RpcError`. `Into::into` calls the
    // `From<E: Error> for RpcError` blanket so any domain error implementing `toni::Error` lifts.
    let message_arms: Vec<_> = message_handlers
        .iter()
        .map(|(pattern, method)| {
            let method_name = &method.sig.ident;
            let (extractions, call_args) = handler_params(method);
            if returns_rpc_handler_output(method) {
                quote! {
                    #pattern => {
                        #(#extractions)*
                        match self.#method_name(#(#call_args),*).await {
                            Ok(__output) => ::toni::http_helpers::ExecutionResult::Ok(__output),
                            Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                                ::std::convert::Into::<::toni::rpc::RpcError>::into(__err),
                            ),
                        }
                    }
                }
            } else if returns_rpc_data(method) {
                quote! {
                    #pattern => {
                        #(#extractions)*
                        match self.#method_name(#(#call_args),*).await {
                            Ok(__data) => ::toni::http_helpers::ExecutionResult::Ok(
                                ::toni::rpc::RpcHandlerOutput::Single(__data),
                            ),
                            Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                                ::std::convert::Into::<::toni::rpc::RpcError>::into(__err),
                            ),
                        }
                    }
                }
            } else {
                quote! {
                    #pattern => {
                        #(#extractions)*
                        match self.#method_name(#(#call_args),*).await {
                            Ok(__result) => match ::toni::rpc::RpcData::from_serialize(&__result) {
                                Ok(__data) => ::toni::http_helpers::ExecutionResult::Ok(
                                    ::toni::rpc::RpcHandlerOutput::Single(__data),
                                ),
                                Err(__e) => ::toni::http_helpers::ExecutionResult::Err(
                                    ::toni::rpc::RpcError::Internal(__e.to_string()),
                                ),
                            },
                            Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                                ::std::convert::Into::<::toni::rpc::RpcError>::into(__err),
                            ),
                        }
                    }
                }
            }
        })
        .collect();

    let event_arms: Vec<_> = event_handlers
        .iter()
        .map(|(pattern, method)| {
            let method_name = &method.sig.ident;
            let (extractions, call_args) = handler_params(method);
            quote! {
                #pattern => {
                    #(#extractions)*
                    match self.#method_name(#(#call_args),*).await {
                        Ok(()) => ::toni::http_helpers::ExecutionResult::Ok(
                            ::toni::rpc::RpcHandlerOutput::Empty,
                        ),
                        Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                            ::std::convert::Into::<::toni::rpc::RpcError>::into(__err),
                        ),
                    }
                }
            }
        })
        .collect();

    let enhancers_impl = build_enhancers_fn(&impl_block, &message_handlers, &event_handlers)?;
    let metadata_fns = build_metadata_fns(&impl_block, &message_handlers, &event_handlers)?;

    // Re-emit the impl with the consumed pattern markers and enhancer attrs stripped. `#[new]` and
    // the `#[on_*]` lifecycle attrs are LEFT intact so their own macros form the bridges that
    // `#[controller]`'s wiring dispatches through.
    let mut impl_def = impl_block.clone();
    impl_def
        .attrs
        .retain(|attr| !has_enhancer_attribute(attr) && !attr_is(attr, "set_metadata"));
    for item in impl_def.items.iter_mut() {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr_is(attr, "message_pattern")
                    && !attr_is(attr, "event_pattern")
                    && !attr_is(attr, "set_metadata")
                    && !has_enhancer_attribute(attr)
            });
        }
    }

    let struct_token = struct_name.to_string();
    let source_name = rpc_source_ident(&struct_name);

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        impl #struct_name {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            fn __toni_rpc_patterns() -> Vec<String> {
                vec![#(#all_patterns.to_string()),*]
            }

            /// Shadows the `DispatchBridge` default: this controller dispatches RPC patterns.
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            pub fn __toni_dispatch(
                source: &::toni::traits_helpers::DispatchSource<#struct_name>,
            ) -> ::toni::traits_helpers::Dispatch {
                // The route prefix is HTTP's argument; patterns cannot use one.
                if !<#struct_name>::__toni_prefix().is_empty() {
                    ::toni::tracing::warn!(
                        controller = #struct_token,
                        prefix = <#struct_name>::__toni_prefix(),
                        "controller dispatches RPC patterns; the route prefix is unused"
                    );
                }
                ::toni::traits_helpers::Dispatch::Rpc(
                    ::std::sync::Arc::new(#source_name(source.clone())),
                )
            }

            #metadata_fns

            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            async fn __toni_rpc_handle_message(
                &self,
                ctx: &::toni::context::RpcContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::toni::rpc::RpcHandlerOutput,
                ::toni::rpc::RpcError,
            > {
                let data = ctx.data().clone();
                let _ = &data;
                match ctx.pattern() {
                    #(#message_arms)*
                    #(#event_arms)*
                    _ => ::toni::http_helpers::ExecutionResult::Err(
                        ::toni::rpc::RpcError::PatternNotFound(
                            format!("Unknown pattern: {}", ctx.pattern()),
                        ),
                    ),
                }
            }

            #enhancers_impl
        }

        #[::toni::async_trait]
        impl ::toni::rpc::RpcControllerTrait for #struct_name {
            async fn handle_message(
                &self,
                ctx: &::toni::context::RpcContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::toni::rpc::RpcHandlerOutput,
                ::toni::rpc::RpcError,
            > {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <Self>::__toni_rpc_handle_message(self, ctx).await
            }
        }

        #[doc(hidden)]
        pub struct #source_name(::toni::traits_helpers::DispatchSource<#struct_name>);

        #[::toni::async_trait]
        impl ::toni::rpc::RpcControllerSource for #source_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_patterns(&self) -> Vec<String> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_patterns()
            }

            fn enhancers(&self) -> ::toni::rpc::RpcEnhancers {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_enhancers()
            }

            fn metadata(&self) -> ::std::sync::Arc<::toni::context::Metadata> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                ::std::sync::Arc::new(<#struct_name>::__toni_rpc_metadata())
            }

            fn handler_metadata(
                &self,
            ) -> Vec<(String, ::std::sync::Arc<::toni::context::Metadata>)> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_handler_metadata()
                    .into_iter()
                    .map(|(__p, __m)| (__p, ::std::sync::Arc::new(__m)))
                    .collect()
            }

            async fn instance(
                &self,
                ctx: &::toni::context::RpcContext,
            ) -> ::std::sync::Arc<dyn ::toni::rpc::RpcControllerTrait> {
                self.0
                    .instance(::toni::ProviderContext::Rpc(ctx.clone()))
                    .await
            }
        }
    })
}

/// The `RpcControllerSource` companion generated beside the controller — a newtype over
/// `DispatchSource<Struct>`, local to the expansion crate because a foreign trait cannot be
/// implemented on the foreign `DispatchSource` directly.
fn rpc_source_ident(struct_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &format!("{}RpcControllerSource", struct_name),
        struct_name.span(),
    )
}

/// Collect controller-level and per-handler enhancer tokens into the `__toni_rpc_enhancers` inherent
/// fn, which shadows the bridge default and builds the `RpcEnhancers` descriptor the resolver reads.
fn build_enhancers_fn(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
    event_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_infos = create_enhancer_infos(ctrl_enhancers_attr, Vec::new())?;

    let tokens_for = |infos: &std::collections::HashMap<
        String,
        Vec<crate::enhancer::enhancer::EnhancerInfo>,
    >,
                      key: &str|
     -> Vec<TokenStream> {
        let empty = Vec::new();
        infos
            .get(key)
            .unwrap_or(&empty)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect()
    };

    let guard_tokens = tokens_for(&ctrl_infos, "guards");
    let interceptor_tokens = tokens_for(&ctrl_infos, "interceptors");
    let error_handler_tokens = tokens_for(&ctrl_infos, "error_handlers");

    let mut handler_entries: Vec<TokenStream> = Vec::new();
    for (pattern, method) in message_handlers.iter().chain(event_handlers.iter()) {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let infos = create_enhancer_infos(method_enhancers_attr, Vec::new())?;
        let hg = tokens_for(&infos, "guards");
        let hi = tokens_for(&infos, "interceptors");
        let he = tokens_for(&infos, "error_handlers");
        if hg.is_empty() && hi.is_empty() && he.is_empty() {
            continue;
        }
        handler_entries.push(quote! {
            ::toni::rpc::RpcHandlerEnhancers {
                pattern: #pattern.to_string(),
                guard_tokens: vec![#(#hg),*],
                interceptor_tokens: vec![#(#hi),*],
                error_handler_tokens: vec![#(#he),*],
            }
        });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __toni_rpc_enhancers() -> ::toni::rpc::RpcEnhancers {
            ::toni::rpc::RpcEnhancers {
                guard_tokens: vec![#(#guard_tokens),*],
                interceptor_tokens: vec![#(#interceptor_tokens),*],
                error_handler_tokens: vec![#(#error_handler_tokens),*],
                handlers: vec![#(#handler_entries),*],
            }
        }
    })
}

fn extract_pattern_attr(attrs: &[Attribute], name: &str) -> Option<String> {
    for attr in attrs {
        if attr_is(attr, name) {
            if let Ok(lit) = attr.parse_args::<LitStr>() {
                return Some(lit.value());
            }
        }
    }
    None
}

/// Enforces that `#[event_pattern]` handlers return `Result<(), RpcError>`. Without this the Ok value
/// is silently dropped by the generated arm — a confusing bug if `Result<RpcData, RpcError>` slips in.
fn check_event_return_type(method: &syn::ImplItemFn) -> Result<()> {
    let syn::ReturnType::Type(_, ty) = &method.sig.output else {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "#[event_pattern] handler must return `Result<(), RpcError>`",
        ));
    };

    if let syn::Type::Path(type_path) = ty.as_ref() {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(first)) = args.args.first() {
                        if let syn::Type::Tuple(tuple) = first {
                            if tuple.elems.is_empty() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }

    Err(syn::Error::new_spanned(
        &method.sig.output,
        "#[event_pattern] handler must return `Result<(), RpcError>` — use `#[message_pattern]` to return data",
    ))
}

/// True if the type path ends in `RpcData`.
fn is_rpc_data(ty: &syn::Type) -> bool {
    if let syn::Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == "RpcData";
        }
    }
    false
}

/// Extraction for a handler's parameters, in signature order.
///
/// Anything the framework knows — `RpcData`, `Extensions`, `Payload<T>`,
/// `Validated<Payload<T>>` — is a `FromContext<RpcContext>`. A parameter of any
/// other type is the call's payload, deserialised into it: the convention RPC
/// handlers have always used, now one case among several rather than the only
/// shape a handler can take.
///
/// `&RpcContext` passes through, reborrowed at the call so
/// they hold no borrow across the extractions before them.
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

        if is_rpc_context_ref(ty) {
            call_args.push(quote! { &*ctx });
            continue;
        }

        let extraction = if is_known_extractor(ty) {
            quote! {
                let #name = match <#ty as ::toni::extractors::FromContext<
                    ::toni::context::RpcContext,
                >>::extract(ctx).await {
                    ::std::result::Result::Ok(__value) => __value,
                    ::std::result::Result::Err(__e) => {
                        return ::toni::http_helpers::ExecutionResult::Err(
                            ::toni::rpc::RpcError::Internal(__e.to_string()),
                        );
                    }
                };
            }
        } else {
            // The bare-payload convention: deserialise the call's data into it.
            quote! {
                let #name = match ctx.data().parse::<#ty>() {
                    ::std::result::Result::Ok(__value) => __value,
                    ::std::result::Result::Err(__e) => {
                        return ::toni::http_helpers::ExecutionResult::Err(
                            ::toni::rpc::RpcError::Internal(__e.to_string()),
                        );
                    }
                };
            }
        };
        extractions.push(extraction);
        call_args.push(quote! { #name });
    }

    (extractions, call_args)
}

/// `&RpcContext` — the context itself, not something
/// extracted from it.
fn is_rpc_context_ref(ty: &syn::Type) -> bool {
    let syn::Type::Reference(type_ref) = ty else {
        return false;
    };
    matches!(&*type_ref.elem, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "RpcContext"))
}

/// Types with a `FromContext<RpcContext>` impl in the framework. Everything else
/// is the payload.
fn is_known_extractor(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = ty else {
        return false;
    };
    type_path.path.segments.last().is_some_and(|s| {
        matches!(
            s.ident.to_string().as_str(),
            "RpcData" | "Extensions" | "Payload" | "Validated"
        )
    })
}

/// True when a `#[message_pattern]` handler answers with `RpcHandlerOutput` itself — declared as
/// `-> RpcHandlerResult` or `-> Result<RpcHandlerOutput, E>` — so the generated arm passes the
/// output through untouched. Checked before [`returns_rpc_data`], which reads any unrecognized
/// return shape as its passthrough case.
fn returns_rpc_handler_output(method: &syn::ImplItemFn) -> bool {
    let syn::ReturnType::Type(_, ty) = &method.sig.output else {
        return false;
    };
    let syn::Type::Path(tp) = ty.as_ref() else {
        return false;
    };
    let Some(seg) = tp.path.segments.last() else {
        return false;
    };
    if seg.ident == "RpcHandlerResult" {
        return true;
    }
    if seg.ident != "Result" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return false;
    };
    matches!(inner, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "RpcHandlerOutput"))
}

/// True when a `#[message_pattern]` handler's `Ok` arm is `RpcData` (forwarded as-is); any other `T`
/// is serialized via `RpcData::from_serialize`.
fn returns_rpc_data(method: &syn::ImplItemFn) -> bool {
    let syn::ReturnType::Type(_, ty) = &method.sig.output else {
        return true;
    };
    let syn::Type::Path(tp) = ty.as_ref() else {
        return true;
    };
    let Some(seg) = tp.path.segments.last() else {
        return true;
    };
    if seg.ident != "Result" {
        return true;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return true;
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return true;
    };
    is_rpc_data(inner)
}

/// The controller's declared metadata: the impl block's entries as the base, and one merged entry per
/// pattern whose handler adds to them. A pattern the handler does not annotate reads the base.
fn build_metadata_fns(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
    event_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let base = metadata_ctor(&get_metadata_exprs(&impl_block.attrs)?)
        .unwrap_or_else(|| quote! { ::toni::context::Metadata::new() });

    let mut entries: Vec<TokenStream> = Vec::new();
    for (pattern, method) in message_handlers.iter().chain(event_handlers.iter()) {
        if get_metadata_exprs(&method.attrs)?.is_empty() {
            continue;
        }
        let merged = merged_metadata_exprs(&impl_block.attrs, &method.attrs)?;
        let ctor = metadata_ctor(&merged).expect("non-empty");
        entries.push(quote! { (#pattern.to_string(), #ctor) });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __toni_rpc_metadata() -> ::toni::context::Metadata {
            #base
        }

        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __toni_rpc_handler_metadata()
            -> Vec<(String, ::toni::context::Metadata)> {
            vec![#(#entries),*]
        }
    })
}
