//! `#[patterns]` — the impl-side pattern router for an RPC controller.
//!
//! Pairs with `#[controller]` on the struct, and is what makes the controller RPC: it emits the
//! `__ulo_dispatch` shadow answering `Dispatch::Rpc`, the `RpcController` impl, and the
//! source companion. It scans the impl for `#[message_pattern]` (request-response) and
//! `#[event_pattern]` (fire-and-forget) handlers and the controller- and handler-level enhancer
//! attrs, and emits inherent `__ulo_rpc_*` fns that out-rank the `RpcHandlersBridge` defaults at
//! the concrete-type call sites in the `RpcController` impl. RPC has no connection hooks, so
//! the scan is pure aggregation. It leaves `#[new]` and `#[on_*]` intact for their own macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, ImplItem, ItemImpl, LitStr, Result, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, declares_anything, enhancer_entries, get_enhancers_attr,
    has_enhancer_attribute,
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
    // `From<E: Error> for RpcError` blanket so any domain error implementing `ulo::Error` lifts.
    let message_arms: Vec<_> = message_handlers
        .iter()
        .map(|(pattern, method)| {
            let method_name = &method.sig.ident;
            let (extractions, call_args) = handler_params(method);
            quote! {
                #pattern => {
                    #(#extractions)*
                    use ::ulo::__rpc::answer::{Answered as _, Serialized as _};
                    match self.#method_name(#(#call_args),*).await {
                        Ok(__value) => {
                            let __answers = ::ulo::__rpc::answer::Answers::new(__value);
                            match (&&__answers).ulo_answer() {
                                Ok(__output) => ::ulo::dispatch::ExecutionResult::Ok(__output),
                                Err(__err) => ::ulo::dispatch::ExecutionResult::Err(__err),
                            }
                        }
                        Err(__err) => ::ulo::dispatch::ExecutionResult::Err(
                            ::std::convert::Into::<::ulo::rpc::RpcError>::into(__err),
                        ),
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
                        Ok(()) => ::ulo::dispatch::ExecutionResult::Ok(
                            ::ulo::dispatch::Items::Empty,
                        ),
                        Err(__err) => ::ulo::dispatch::ExecutionResult::Err(
                            ::std::convert::Into::<::ulo::rpc::RpcError>::into(__err),
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
            fn __ulo_rpc_patterns() -> Vec<String> {
                vec![#(#all_patterns.to_string()),*]
            }

            /// Shadows the `DispatchBridge` default: this controller dispatches RPC patterns.
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            pub fn __ulo_dispatch(
                source: &::ulo::__enhancer::DispatchSource<#struct_name>,
            ) -> ::ulo::dispatch::Targets {
                // The route prefix is HTTP's argument; patterns cannot use one.
                if !<#struct_name>::__ulo_prefix().is_empty() {
                    ::ulo::tracing::warn!(
                        controller = #struct_token,
                        prefix = <#struct_name>::__ulo_prefix(),
                        "controller dispatches RPC patterns; the route prefix is unused"
                    );
                }
                ::ulo::dispatch::Targets::Rpc(
                    ::std::sync::Arc::new(#source_name(source.clone())),
                )
            }

            #metadata_fns

            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            async fn __ulo_rpc_handle_message(
                &self,
                ctx: &::ulo::rpc::RpcContext,
            ) -> ::ulo::dispatch::ExecutionResult<
                ::ulo::rpc::RpcHandlerOutput,
                ::ulo::rpc::RpcError,
            > {
                let data = ctx.data().clone();
                let _ = &data;
                match ctx.pattern() {
                    #(#message_arms)*
                    #(#event_arms)*
                    // A typed event, so a `#[catch(Unrouted)]` handler can claim
                    // it. This arm answers a pattern the controller was
                    // registered for but has no method behind; the dispatcher
                    // raises the same event for a pattern no controller claims.
                    _ => ::ulo::dispatch::ExecutionResult::Err(
                        ::ulo::rpc::RpcError::AppError(::std::sync::Arc::new(
                            ::ulo::errors::Unrouted::new(ctx.pattern()),
                        )),
                    ),
                }
            }

            #enhancers_impl
        }

        #[::ulo::async_trait]
        impl ::ulo::rpc::RpcController for #struct_name {
            async fn handle_message(
                &self,
                ctx: &::ulo::rpc::RpcContext,
            ) -> ::ulo::dispatch::ExecutionResult<
                ::ulo::rpc::RpcHandlerOutput,
                ::ulo::rpc::RpcError,
            > {
                use ::ulo::__rpc::RpcHandlersBridge as _;
                <Self>::__ulo_rpc_handle_message(self, ctx).await
            }
        }

        #[doc(hidden)]
        pub struct #source_name(::ulo::__enhancer::DispatchSource<#struct_name>);

        #[::ulo::async_trait]
        impl ::ulo::rpc::RpcControllerSource for #source_name {
            fn token(&self) -> String {
                #struct_token.to_string()
            }

            fn patterns(&self) -> Vec<String> {
                use ::ulo::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__ulo_rpc_patterns()
            }

            fn enhancers(&self) -> ::ulo::rpc::RpcEnhancers {
                use ::ulo::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__ulo_rpc_enhancers()
            }

            fn metadata(&self) -> ::std::sync::Arc<::ulo::context::Metadata> {
                use ::ulo::__rpc::RpcHandlersBridge as _;
                ::std::sync::Arc::new(<#struct_name>::__ulo_rpc_metadata())
            }

            fn handler_metadata(
                &self,
            ) -> Vec<(String, ::std::sync::Arc<::ulo::context::Metadata>)> {
                use ::ulo::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__ulo_rpc_handler_metadata()
                    .into_iter()
                    .map(|(__p, __m)| (__p, ::std::sync::Arc::new(__m)))
                    .collect()
            }

            async fn resolve(
                &self,
                ctx: &::ulo::rpc::RpcContext,
            ) -> ::std::result::Result<
                ::std::sync::Arc<dyn ::ulo::rpc::RpcController>,
                ::ulo::errors::HookFailed,
            > {
                let __controller: ::std::sync::Arc<dyn ::ulo::rpc::RpcController> = self
                    .0
                    .resolve(::ulo::di::Execution::Rpc(ctx.clone()))
                    .await?;
                ::std::result::Result::Ok(__controller)
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

/// Collect controller-level and per-handler enhancer tokens into the `__ulo_rpc_enhancers` inherent
/// fn, which shadows the bridge default and builds the `RpcEnhancers` descriptor the resolver reads.
fn build_enhancers_fn(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
    event_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let transport = quote! { ::ulo::dispatch::Rpc };
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_infos = create_enhancer_infos(ctrl_enhancers_attr, Vec::new())?;
    let guards = enhancer_entries(&ctrl_infos, "guards", &transport);
    let interceptors = enhancer_entries(&ctrl_infos, "interceptors", &transport);
    let error_handlers = enhancer_entries(&ctrl_infos, "error_handlers", &transport);

    let mut handler_entries: Vec<TokenStream> = Vec::new();
    for (pattern, method) in message_handlers.iter().chain(event_handlers.iter()) {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let infos = create_enhancer_infos(method_enhancers_attr, Vec::new())?;
        if !declares_anything(&infos) {
            continue;
        }
        let hg = enhancer_entries(&infos, "guards", &transport);
        let hi = enhancer_entries(&infos, "interceptors", &transport);
        let he = enhancer_entries(&infos, "error_handlers", &transport);
        handler_entries.push(quote! {
            ::ulo::rpc::RpcHandlerEnhancers {
                pattern: #pattern.to_string(),
                guards: vec![#(#hg),*],
                interceptors: vec![#(#hi),*],
                error_handlers: vec![#(#he),*],
            }
        });
    }

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __ulo_rpc_enhancers() -> ::ulo::rpc::RpcEnhancers {
            ::ulo::rpc::RpcEnhancers {
                guards: vec![#(#guards),*],
                interceptors: vec![#(#interceptors),*],
                error_handlers: vec![#(#error_handlers),*],
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
/// Extraction for a handler's parameters, in signature order.
///
/// Every parameter is a `FromContext<RpcContext>`, so a handler takes what it
/// needs and the message arrives through an extractor that says so —
/// `Payload<T>` for the call's data (ADR-0041).
///
/// `&RpcContext` passes through, reborrowed at the call so it holds no borrow
/// across the extractions before it.
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

        let extraction = quote! {
            let #name = match <#ty as ::ulo::extract::FromContext<
                ::ulo::rpc::RpcContext,
            >>::extract(ctx).await {
                ::std::result::Result::Ok(__value) => __value,
                ::std::result::Result::Err(__e) => {
                    return ::ulo::dispatch::ExecutionResult::Err(
                        ::ulo::rpc::RpcError::Internal(__e.to_string()),
                    );
                }
            };
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

/// The controller's declared metadata: the impl block's entries as the base, and one merged entry per
/// pattern whose handler adds to them. A pattern the handler does not annotate reads the base.
fn build_metadata_fns(
    impl_block: &ItemImpl,
    message_handlers: &[(String, syn::ImplItemFn)],
    event_handlers: &[(String, syn::ImplItemFn)],
) -> Result<TokenStream> {
    let base = metadata_ctor(&get_metadata_exprs(&impl_block.attrs)?)
        .unwrap_or_else(|| quote! { ::ulo::context::Metadata::new() });

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
        fn __ulo_rpc_metadata() -> ::ulo::context::Metadata {
            #base
        }

        #[doc(hidden)]
        #[allow(non_snake_case, clippy::all)]
        fn __ulo_rpc_handler_metadata()
            -> Vec<(String, ::ulo::context::Metadata)> {
            vec![#(#entries),*]
        }
    })
}
