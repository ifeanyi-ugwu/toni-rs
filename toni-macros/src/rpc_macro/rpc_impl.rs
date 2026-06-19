use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashSet;
use syn::{Attribute, ItemImpl, ItemStruct, LitStr, Result, parse2};

use crate::controller_macro::controller_struct::{extract_constructor_params, has_new_method};
use crate::enhancer::enhancer::{
    create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute,
};
use crate::provider_macro::instance_injection::generate_instance_provider_system;
use crate::shared::attr_is;
use crate::shared::dependency_info::DependencySource;
use crate::shared::scope_parser::ProviderScope;
use crate::utils::extracts::extract_struct_dependencies;

/// Parse `#[rpc_controller]` or `#[rpc_controller(pub struct Foo { ... })]`.
struct RpcControllerArgs {
    /// `None` when the struct is defined above the impl.
    struct_def: Option<ItemStruct>,
}

impl syn::parse::Parse for RpcControllerArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let struct_def = if !input.is_empty() {
            Some(input.parse::<ItemStruct>()?)
        } else {
            None
        };
        Ok(RpcControllerArgs { struct_def })
    }
}

pub fn handle_rpc_controller(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<RpcControllerArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let struct_def = args.struct_def;

    let mut dependencies = match &struct_def {
        Some(s) => extract_struct_dependencies(s)?,
        None => crate::shared::dependency_info::DependencyInfo {
            fields: vec![],
            owned_fields: vec![],
            init_method: None,
            constructor_params: vec![],
            unique_types: HashSet::new(),
            source: DependencySource::None,
        },
    };

    if has_new_method(&impl_block) {
        let params = extract_constructor_params(&impl_block, "new")?;
        dependencies.init_method = Some("new".to_string());
        dependencies.constructor_params = params;
        dependencies.source = DependencySource::Constructor("new".to_string());
    } else if struct_def.is_none() {
        return Err(syn::Error::new_spanned(
            &impl_block.self_ty,
            "add a `fn new(...) -> Self` constructor to declare this RPC controller's dependencies, \
             or move the struct definition into the macro attribute",
        ));
    }

    generate_rpc_controller_impl(struct_def.as_ref(), &impl_block, &dependencies)
}

fn generate_rpc_controller_impl(
    struct_def: Option<&ItemStruct>,
    impl_block: &ItemImpl,
    dependencies: &crate::shared::dependency_info::DependencyInfo,
) -> Result<TokenStream> {
    let struct_name = match struct_def {
        Some(s) => s.ident.clone(),
        None => crate::utils::extracts::extract_impl_self_ident(impl_block)?,
    };
    let struct_name = &struct_name;
    let struct_token = struct_name.to_string();

    let mut message_handlers: Vec<(String, syn::ImplItemFn)> = Vec::new();
    let mut event_handlers: Vec<(String, syn::ImplItemFn)> = Vec::new();

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
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

    // User errors flow through the dispatcher as `RpcError`. `Into::into`
    // calls the `From<E: Error> for RpcError` blanket so any domain
    // error type implementing `toni::Error` lifts automatically.
    let message_arms: Vec<_> = message_handlers
        .iter()
        .map(|(pattern, method)| {
            let method_name = &method.sig.ident;
            let (payload_extract, payload_expr) = typed_payload_expr(method);
            if returns_rpc_data(method) {
                quote! {
                    #pattern => {
                        #payload_extract
                        match self.#method_name(#payload_expr, ctx).await {
                            Ok(__data) => ::toni::http_helpers::ExecutionResult::Ok(Some(__data)),
                            Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                                ::std::convert::Into::<toni::rpc::RpcError>::into(__err),
                            ),
                        }
                    }
                }
            } else {
                quote! {
                    #pattern => {
                        #payload_extract
                        match self.#method_name(#payload_expr, ctx).await {
                            Ok(__result) => match toni::rpc::RpcData::from_serialize(&__result) {
                                Ok(__data) => ::toni::http_helpers::ExecutionResult::Ok(Some(__data)),
                                Err(__e) => ::toni::http_helpers::ExecutionResult::Err(
                                    toni::rpc::RpcError::Internal(__e.to_string()),
                                ),
                            },
                            Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                                ::std::convert::Into::<toni::rpc::RpcError>::into(__err),
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
            let (payload_extract, payload_expr) = typed_payload_expr(method);
            quote! {
                #pattern => {
                    #payload_extract
                    match self.#method_name(#payload_expr, ctx).await {
                        Ok(()) => ::toni::http_helpers::ExecutionResult::Ok(None),
                        Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                            ::std::convert::Into::<toni::rpc::RpcError>::into(__err),
                        ),
                    }
                }
            }
        })
        .collect();

    // Extract controller-level enhancer tokens from the impl block attrs
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_enhancer_infos =
        create_enhancer_infos(ctrl_enhancers_attr, std::collections::HashMap::new())?;

    let binding = Vec::new();
    let ctrl_guard_tokens: Vec<_> = ctrl_enhancer_infos
        .get("guards")
        .unwrap_or(&binding)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();
    let ctrl_interceptor_tokens: Vec<_> = ctrl_enhancer_infos
        .get("interceptors")
        .unwrap_or(&binding)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();
    let ctrl_pipe_tokens: Vec<_> = ctrl_enhancer_infos
        .get("pipes")
        .unwrap_or(&binding)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();
    let ctrl_error_handler_tokens: Vec<_> = ctrl_enhancer_infos
        .get("error_handlers")
        .unwrap_or(&binding)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();

    // Extract per-handler enhancer tokens from each #[message_pattern]/#[event_pattern] method
    let all_handlers = message_handlers.iter().chain(event_handlers.iter());
    let mut handler_enhancer_entries: Vec<(
        String,
        Vec<TokenStream>,
        Vec<TokenStream>,
        Vec<TokenStream>,
        Vec<TokenStream>,
    )> = Vec::new();

    for (pattern, method) in all_handlers {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let handler_infos =
            create_enhancer_infos(method_enhancers_attr, std::collections::HashMap::new())?;
        let binding = Vec::new();
        let hg: Vec<TokenStream> = handler_infos
            .get("guards")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let hi: Vec<TokenStream> = handler_infos
            .get("interceptors")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let hp: Vec<TokenStream> = handler_infos
            .get("pipes")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let he: Vec<TokenStream> = handler_infos
            .get("error_handlers")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        if !hg.is_empty() || !hi.is_empty() || !hp.is_empty() || !he.is_empty() {
            handler_enhancer_entries.push((pattern.clone(), hg, hi, hp, he));
        }
    }

    // Collapse controller-level + per-handler tokens into a single `enhancers()` descriptor
    // (replacing a dozen accessor methods). The resolver reads it once at startup.
    let handler_entries: Vec<TokenStream> = handler_enhancer_entries
        .iter()
        .map(|(pattern, hg, hi, hp, he)| {
            quote! {
                ::toni::RpcHandlerEnhancers {
                    pattern: #pattern.to_string(),
                    guard_tokens: vec![#(#hg),*],
                    interceptor_tokens: vec![#(#hi),*],
                    pipe_tokens: vec![#(#hp),*],
                    error_handler_tokens: vec![#(#he),*],
                }
            }
        })
        .collect();

    let enhancers_impl = quote! {
        fn enhancers(&self) -> ::toni::RpcEnhancers {
            ::toni::RpcEnhancers {
                guard_tokens: vec![#(#ctrl_guard_tokens),*],
                interceptor_tokens: vec![#(#ctrl_interceptor_tokens),*],
                pipe_tokens: vec![#(#ctrl_pipe_tokens),*],
                error_handler_tokens: vec![#(#ctrl_error_handler_tokens),*],
                handlers: vec![#(#handler_entries),*],
            }
        }
    };

    // Strip marker and enhancer attributes from the impl block before emitting it
    let mut impl_def = impl_block.clone();
    impl_def.attrs.retain(|attr| !has_enhancer_attribute(attr));
    for item in impl_def.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr_is(attr, "message_pattern")
                    && !attr_is(attr, "event_pattern")
                    && !has_enhancer_attribute(attr)
            });
        }
    }

    let provider_system = generate_instance_provider_system(
        struct_def,
        &impl_def,
        dependencies,
        ProviderScope::Singleton,
        false, // is_gateway
        true,  // is_rpc_controller
        false, // is_grpc_service
    )?;

    let rpc_trait_impl = quote! {
        #[toni::async_trait]
        impl toni::rpc::RpcControllerTrait for #struct_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_patterns(&self) -> Vec<String> {
                vec![#(#all_patterns.to_string()),*]
            }

            #enhancers_impl

            async fn handle_message(
                &self,
                ctx: &toni::context::RpcContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                Option<toni::rpc::RpcData>,
                toni::rpc::RpcError,
            > {
                let data = ctx.data().clone();
                let _ = &data;
                match ctx.pattern() {
                    #(#message_arms)*
                    #(#event_arms)*
                    _ => ::toni::http_helpers::ExecutionResult::Err(
                        toni::rpc::RpcError::PatternNotFound(
                            format!("Unknown pattern: {}", ctx.pattern()),
                        ),
                    ),
                }
            }
        }
    };

    Ok(quote! {
        #provider_system

        #rpc_trait_impl
    })
}

/// Enforces that `#[event_pattern]` handlers return `Result<(), RpcError>`.
///
/// Without this check the Ok value is silently discarded by the generated match arm,
/// which would be a confusing silent bug if the user accidentally used `Result<RpcData, RpcError>`.
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

/// Returns true if the type path ends in `RpcData`.
fn is_rpc_data(ty: &syn::Type) -> bool {
    if let syn::Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == "RpcData";
        }
    }
    false
}

/// Generates the expression passed as the first non-self argument to the handler.
///
/// If the declared parameter type is `RpcData` (or a path ending in it), the
/// raw `data` value is forwarded directly.  Otherwise, the payload is
/// deserialized into the declared type so handlers can use concrete structs.
/// Emit the per-arm payload extraction.
///
/// Returns `(extract_stmt, payload_expr)`:
/// - `extract_stmt` is either empty (untyped — payload is the raw `data`)
///   or a `let __payload = ...` that early-returns
///   `ExecutionResult::Err(RpcError::Internal)` if parsing fails.
/// - `payload_expr` is what the macro passes to the user method —
///   `data` for untyped, `__payload` for typed.
///
/// Splitting this way avoids the `?` operator (which doesn't work
/// against `ExecutionResult`) without losing the parse-error path.
fn typed_payload_expr(
    method: &syn::ImplItemFn,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let payload_ty = method.sig.inputs.iter().find_map(|arg| {
        if let syn::FnArg::Typed(pt) = arg {
            Some(pt.ty.as_ref())
        } else {
            None
        }
    });

    match payload_ty {
        Some(ty) if !is_rpc_data(ty) => {
            let extract = quote! {
                let __payload = match data.parse::<#ty>() {
                    Ok(__p) => __p,
                    Err(__e) => {
                        return ::toni::http_helpers::ExecutionResult::Err(
                            toni::rpc::RpcError::Internal(__e.to_string()),
                        );
                    }
                };
            };
            (extract, quote! { __payload })
        }
        _ => (proc_macro2::TokenStream::new(), quote! { data }),
    }
}

/// Returns true when a `#[message_pattern]` handler's `Ok` arm contains `RpcData`.
///
/// Handlers that return `Result<RpcData, RpcError>` are forwarded as-is.
/// Handlers that return `Result<T, RpcError>` for any other T are serialized
/// via `RpcData::from_serialize`.
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
