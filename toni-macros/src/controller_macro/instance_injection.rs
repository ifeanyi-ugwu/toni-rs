//! `#[routes]` impl-side controller codegen.
//!
//! `#[controller("/p")]` on the struct ([controller_attr]) emits the DI bridges
//! (`__toni_build_from_deps` / `__toni_dependencies` / `__toni_prefix` / `__toni_is_request_scoped`).
//! `#[routes]` on the impl — this module — scans the handler methods and emits one `Route` wrapper
//! per handler method plus the shadowing `__toni_dispatch` that answers `Dispatch::Http` with
//! them, built around the controller's `DispatchSource`. Each wrapper resolves its instance
//! through the source at call time, so one wrapper set serves both the shared-singleton and the
//! built-per-request controller.
//!
//! The two sides never see each other's item; they meet at the concrete type through the inherent
//! bridge fns. A missing `#[controller]` struct surfaces as "no associated function `__toni_build_from_deps`".

use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashMap;
use syn::{Attribute, Error, Ident, ImplItemFn, ItemImpl, LitStr, Result, spanned::Spanned};

use crate::{
    controller_macro::extractor_params::{
        ExtractorKind, generate_extractor_extractions, generate_extractor_method_call,
        generate_extractor_static_method_call, get_extractor_params, has_self_receiver,
    },
    enhancer::enhancer::{EnhancerInfo, create_enhancer_infos},
    markers_params::{
        extracts_marker_params::{
            extract_body_from_param, extract_path_param_from_param, extract_query_from_param,
        },
        get_marker_params::MarkerParam,
    },
    shared::set_metadata::get_metadata_exprs,
    shared::{attr_is, metadata_info::MetadataInfo},
    utils::controller_utils::attr_to_string,
};

/// Entry for `#[routes] impl Foo { … }`. The struct (and its DI bridges) is declared separately via
/// `#[controller]`; this never sees the struct's fields.
pub fn generate_routes_system(impl_block: &ItemImpl) -> Result<TokenStream> {
    let struct_name = crate::utils::extracts::extract_impl_self_ident(impl_block)?;
    let struct_name = &struct_name;

    // Re-emit the impl with the inert param markers and the consumed enhancer attrs stripped —
    // the standalone enhancer macros reject unconsumed use, so none may survive to the output.
    // `#[new]` and the `#[on_*]` lifecycle attrs are LEFT intact so their own macros expand into
    // the `__toni_ctor_*` / `__toni_lc_*` bridges that `#[controller]`'s factory and object
    // dispatch through.
    let mut impl_def = impl_block.clone();
    impl_def
        .attrs
        .retain(|attr| !crate::enhancer::enhancer::has_enhancer_attribute(attr));
    for item in impl_def.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            method
                .attrs
                .retain(|attr| !crate::enhancer::enhancer::has_enhancer_attribute(attr));
            crate::markers_params::remove_marker_controller_fn::remove_marker_in_controller_fn_args(
                method,
            );
        }
    }

    // One wrapper set serves both scopes: each wrapper holds the controller's `DispatchSource`
    // and resolves its instance at call time. Construction and the route prefix are delegated to
    // the struct bridges.
    let (wrappers, metadata) = generate_controller_wrappers(impl_block, struct_name)?;

    let toni_dispatch = generate_toni_dispatch(struct_name, &metadata);

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        #(#wrappers)*

        #toni_dispatch
    })
}

/// Emit the controller's inherent `__toni_dispatch`, which shadows the `DispatchBridge` default
/// and names HTTP: one route wrapper per handler, each holding a clone of the controller's source.
fn generate_toni_dispatch(struct_name: &Ident, metadata: &[MetadataInfo]) -> TokenStream {
    let route_ty = quote! { ::std::sync::Arc<dyn ::toni::traits_helpers::Route> };

    let creations: Vec<_> = metadata
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            if metadata.is_static {
                quote! { ::std::sync::Arc::new(#controller_name {}) as #route_ty }
            } else {
                quote! { ::std::sync::Arc::new(#controller_name { source: source.clone() }) as #route_ty }
            }
        })
        .collect();

    quote! {
        impl #struct_name {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            pub fn __toni_dispatch(
                source: &::toni::traits_helpers::DispatchSource<#struct_name>,
            ) -> ::toni::traits_helpers::Dispatch {
                let _ = source;
                ::toni::traits_helpers::Dispatch::Http(vec![#(#creations),*])
            }
        }
    }
}

fn generate_controller_wrappers(
    impl_block: &ItemImpl,
    struct_name: &Ident,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>)> {
    let mut wrappers = Vec::new();
    let mut metadata_list = Vec::new();

    let controller_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let controller_metadata_exprs = get_metadata_exprs(&impl_block.attrs)?;

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            if let Some(http_method_attr) = find_http_method_attr(&method.attrs) {
                let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
                let marker_params = get_marker_params(method)?;

                let (wrapper, metadata) = generate_controller_wrapper(
                    method,
                    struct_name,
                    http_method_attr,
                    controller_enhancers_attr.clone(),
                    method_enhancers_attr,
                    &controller_metadata_exprs,
                    marker_params,
                )?;

                wrappers.push(wrapper);
                metadata_list.push(metadata);
            }
        }
    }

    Ok((wrappers, metadata_list))
}

fn find_http_method_attr(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|attr| {
        attr_is(attr, "get")
            || attr_is(attr, "post")
            || attr_is(attr, "put")
            || attr_is(attr, "delete")
            || attr_is(attr, "patch")
            || attr_is(attr, "head")
            || attr_is(attr, "options")
            || attr_is(attr, "sse")
    })
}

fn get_enhancers_attr(attrs: &[syn::Attribute]) -> Result<Vec<(&Ident, &Attribute)>> {
    use crate::enhancer::enhancer::get_enhancers_attr as get_enhancers;
    get_enhancers(attrs)
}

fn get_marker_params(method: &ImplItemFn) -> Result<Vec<MarkerParam>> {
    use crate::markers_params::get_marker_params::get_marker_params as get_params;
    get_params(method)
}

#[allow(clippy::too_many_arguments)]
fn generate_controller_wrapper(
    method: &ImplItemFn,
    struct_name: &Ident,
    http_method_attr: &Attribute,
    controller_enhancers_attr: Vec<(&Ident, &Attribute)>,
    method_enhancers_attr: Vec<(&Ident, &Attribute)>,
    controller_metadata_exprs: &[TokenStream],
    marker_params: Vec<MarkerParam>,
) -> Result<(TokenStream, MetadataInfo)> {
    let is_sse = attr_is(http_method_attr, "sse");

    let http_method = if is_sse {
        "get".to_string()
    } else {
        attr_to_string(http_method_attr)
            .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?
    };

    // Sub-path only; the controller's prefix is joined at runtime via `__toni_prefix`.
    let route_path_lit = http_method_attr
        .parse_args::<LitStr>()
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?;
    crate::shared::route_path::validate_route_path(&route_path_lit)?;
    let route_path = route_path_lit.value();

    let method_name = &method.sig.ident;
    let controller_name = Ident::new(
        &format!(
            "{}{}Controller",
            struct_name,
            capitalize_first(method_name.to_string()),
        ),
        method_name.span(),
    );

    let is_static_method = !has_self_receiver(method);

    let enhancer_infos = create_enhancer_infos(controller_enhancers_attr, method_enhancers_attr)?;
    // The impl block's entries first, the method's second: a later `insert` shadows an earlier one,
    // so the method wins where both annotate the same type. That is the result Nest reaches by
    // searching `[getHandler(), getClass()]` in order, settled here instead of at every read.
    let mut metadata_exprs = controller_metadata_exprs.to_vec();
    metadata_exprs.extend(get_metadata_exprs(&method.attrs)?);

    let extractor_params = get_extractor_params(method)?;
    let has_extractors = extractor_params
        .iter()
        .any(|p| !matches!(p.kind, ExtractorKind::HttpRequest | ExtractorKind::Unknown));
    let use_extractors = has_extractors || marker_params.is_empty();

    let (method_call, marker_params_extraction) = if use_extractors {
        let (extractions, call_args) = generate_extractor_extractions(&extractor_params)?;
        let method_call = if is_static_method {
            generate_extractor_static_method_call(method, struct_name, &call_args)?
        } else {
            generate_extractor_method_call(method, &call_args)?
        };
        (method_call, extractions)
    } else {
        let method_call =
            generate_method_call(method, &marker_params, struct_name, is_static_method)?;
        let extractions = generate_marker_params_extraction(&marker_params)?;
        (method_call, extractions)
    };

    let method_call = if is_sse {
        if sse_stream_is_fallible(&method.sig.output) {
            quote! { ::toni::Sse::new(#method_call) }
        } else {
            quote! { ::toni::sse(#method_call) }
        }
    } else {
        method_call
    };

    let returns_result = returns_result_type(&method.sig.output);

    let wrapper = generate_route_wrapper(
        &controller_name,
        struct_name,
        &route_path,
        &http_method,
        &method_call,
        &enhancer_infos,
        &marker_params_extraction,
        &metadata_exprs,
        is_static_method,
        returns_result,
    );

    Ok((
        wrapper,
        MetadataInfo {
            struct_name: controller_name,
            dependencies: Vec::new(),
            is_static: is_static_method,
        },
    ))
}

fn generate_method_call(
    method: &ImplItemFn,
    marker_params: &[MarkerParam],
    struct_name: &Ident,
    is_static: bool,
) -> Result<TokenStream> {
    let method_name = &method.sig.ident;
    let is_async = method.sig.asyncness.is_some();

    let mut call_args = Vec::new();
    for input in method.sig.inputs.iter() {
        if let syn::FnArg::Typed(pat_type) = input {
            if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                let param_name = &pat_ident.ident;
                let is_marker = marker_params.iter().any(|mp| mp.param_name == *param_name);
                if is_marker {
                    call_args.push(quote! { #param_name });
                } else if let syn::Type::Path(type_path) = &*pat_type.ty {
                    if let Some(segment) = type_path.path.segments.last() {
                        if segment.ident == "HttpRequest" {
                            call_args.push(quote! { __req });
                        } else {
                            call_args.push(quote! { #param_name });
                        }
                    }
                }
            }
        }
    }

    let call = if is_static {
        quote! { #struct_name::#method_name(#(#call_args),*) }
    } else {
        quote! { controller.#method_name(#(#call_args),*) }
    };

    Ok(if is_async {
        quote! { #call.await }
    } else {
        call
    })
}

fn generate_marker_params_extraction(marker_params: &[MarkerParam]) -> Result<Vec<TokenStream>> {
    let mut extractions = Vec::new();

    for marker_param in marker_params {
        match marker_param.marker_name.as_str() {
            "body" => extractions.push(extract_body_from_param(marker_param)?),
            "query" => extractions.push(extract_query_from_param(marker_param)?),
            "param" => extractions.push(extract_path_param_from_param(marker_param)?),
            _ => {}
        }
    }

    Ok(extractions)
}

// One wrapper per handler method: it holds the controller's `DispatchSource` and resolves the
// instance at call time — a shared singleton answers immediately, a per-call source builds inside
// this request's execution.
#[allow(clippy::too_many_arguments)]
fn generate_route_wrapper(
    controller_name: &Ident,
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    method_call: &TokenStream,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    marker_params_extraction: &[TokenStream],
    metadata_exprs: &[TokenStream],
    is_static_method: bool,
    returns_result: bool,
) -> TokenStream {
    let (struct_fields, resolve_instance) = if is_static_method {
        (quote! {}, quote! {})
    } else {
        (
            quote! {
                source: ::toni::traits_helpers::DispatchSource<#struct_name>,
            },
            // Resolve the instance before the extractors run: a per-call build reads
            // request-scoped dependencies through the context, while a body extractor
            // may move the request out of it.
            quote! {
                let controller = self.source
                    .instance(::toni::ProviderContext::Http(__ctx.clone()))
                    .await;
            },
        )
    };

    let exec_body = exec_body_for(method_call, returns_result);
    let common = route_common_methods(
        struct_name,
        route_path,
        http_method,
        enhancer_infos,
        metadata_exprs,
    );

    quote! {
        struct #controller_name {
            #struct_fields
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Route for #controller_name {
            async fn execute(
                &self,
                __ctx: &::toni::context::HttpContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::toni::http_helpers::HttpResponse,
                ::toni::errors::HttpError,
            > {
                // Cloned, not borrowed: building a request-scoped dependency holds
                // the parts across an await, and the extractions below need the
                // context back exclusively.
                let _req_parts = __ctx.request().clone();

                #resolve_instance

                #(#marker_params_extraction)*

                use ::toni::http_helpers::IntoResponse;
                #exec_body
            }

            #common
        }
    }
}

/// Pull a role's DI tokens and direct-instantiation expressions out of the manifest.
fn enhancer_vecs(
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    key: &str,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let infos = enhancer_infos.get(key);
    let tokens = infos
        .map(|v| {
            v.iter()
                .filter(|i| !i.token_expr.is_empty())
                .map(|i| i.token_expr.clone())
                .collect()
        })
        .unwrap_or_default();
    let instances = infos
        .map(|v| {
            v.iter()
                .filter(|i| !i.instance_expr.is_empty())
                .map(|i| i.instance_expr.clone())
                .collect()
        })
        .unwrap_or_default();
    (tokens, instances)
}

fn enhancers_method(enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>) -> TokenStream {
    let (guard_tokens, guard_instances) = enhancer_vecs(enhancer_infos, "guards");
    let (interceptor_tokens, interceptor_instances) = enhancer_vecs(enhancer_infos, "interceptors");
    let (error_handler_tokens, error_handler_instances) =
        enhancer_vecs(enhancer_infos, "error_handlers");

    quote! {
        fn enhancers(&self) -> ::toni::traits_helpers::ControllerEnhancers {
            ::toni::traits_helpers::ControllerEnhancers {
                guard_tokens: vec![#(#guard_tokens),*],
                interceptor_tokens: vec![#(#interceptor_tokens),*],
                error_handler_tokens: vec![#(#error_handler_tokens),*],
                guards: vec![#(::std::sync::Arc::new(#guard_instances)),*],
                interceptors: vec![#(::std::sync::Arc::new(#interceptor_instances)),*],
                error_handlers: vec![#(::std::sync::Arc::new(#error_handler_instances)),*],
            }
        }
    }
}

/// `get_path` joins the controller's runtime prefix (`__toni_prefix`) with this route's sub-path.
fn get_path_method(struct_name: &Ident, route_path: &str) -> TokenStream {
    quote! {
        fn get_path(&self) -> String {
            ::toni::http_helpers::join_route(#struct_name::__toni_prefix(), #route_path)
        }
    }
}

fn route_common_methods(
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    metadata_exprs: &[TokenStream],
) -> TokenStream {
    let enhancers = enhancers_method(enhancer_infos);
    let get_path = get_path_method(struct_name, route_path);
    quote! {
        fn get_method(&self) -> ::toni::http_helpers::HttpMethod {
            ::toni::http_helpers::HttpMethod::from_string(#http_method).unwrap()
        }

        #get_path

        #enhancers

        fn metadata(&self) -> ::std::sync::Arc<::toni::context::Metadata> {
            let mut metadata = ::toni::context::Metadata::new();
            #(metadata.insert(#metadata_exprs);)*
            ::std::sync::Arc::new(metadata)
        }

    }
}

fn exec_body_for(method_call: &TokenStream, returns_result: bool) -> TokenStream {
    if returns_result {
        quote! {
            match #method_call {
                ::std::result::Result::Ok(__t) => ::toni::http_helpers::ExecutionResult::Ok(
                    ::toni::http_helpers::IntoResponse::into_response(__t),
                ),
                ::std::result::Result::Err(__e) => ::toni::http_helpers::ExecutionResult::Err(
                    ::std::convert::Into::<::toni::errors::HttpError>::into(__e),
                ),
            }
        }
    } else {
        quote! {
            ::toni::http_helpers::ExecutionResult::Ok(
                ::toni::http_helpers::IntoResponse::into_response(#method_call),
            )
        }
    }
}

/// `true` when the user method's return type is `Result<_, _>`.
fn returns_result_type(output: &syn::ReturnType) -> bool {
    if let syn::ReturnType::Type(_, ty) = output
        && let syn::Type::Path(type_path) = ty.as_ref()
    {
        return type_path
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Result");
    }
    false
}

/// `true` when the return type is `impl Stream<Item = Result<_, _>>` — the per-event fallible
/// shape that maps to `Sse::new(stream)` rather than `sse(stream)`.
fn sse_stream_is_fallible(output: &syn::ReturnType) -> bool {
    let syn::ReturnType::Type(_, ty) = output else {
        return false;
    };
    let syn::Type::ImplTrait(impl_trait) = ty.as_ref() else {
        return false;
    };
    for bound in &impl_trait.bounds {
        let syn::TypeParamBound::Trait(tb) = bound else {
            continue;
        };
        let Some(last) = tb.path.segments.last() else {
            continue;
        };
        if last.ident != "Stream" {
            continue;
        }
        let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
            continue;
        };
        for arg in &args.args {
            if let syn::GenericArgument::AssocType(assoc) = arg
                && assoc.ident == "Item"
                && let syn::Type::Path(item_ty) = &assoc.ty
            {
                return item_ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Result");
            }
        }
    }
    false
}

fn capitalize_first(s: String) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}
