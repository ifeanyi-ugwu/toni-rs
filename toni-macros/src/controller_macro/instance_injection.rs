//! `#[routes]` impl-side controller codegen.
//!
//! `#[controller("/p")]` on the struct ([controller_attr]) emits the DI bridges
//! (`__toni_build_from_deps` / `__toni_dependencies` / `__toni_prefix` / `__toni_is_request_scoped`).
//! `#[routes]` on the impl — this module — scans the handler methods and emits:
//!
//! 1. one `Route` wrapper per handler method (singleton + request variants),
//! 2. the `…ControllerObject` (`Controller`) whose `routes()` yields them and whose lifecycle hooks
//!    delegate to the built instance,
//! 3. the `…ControllerFactory` whose `build()` resolves scope/elevation at runtime and constructs the
//!    instance through `Self::__toni_build_from_deps`.
//!
//! The two sides never see each other's item; they meet at the concrete type through those inherent
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
    shared::{attr_is, metadata_info::MetadataInfo, scope_parser::ControllerScope},
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

    // Both wrapper sets are generated; the factory picks the state at runtime and `__toni_routes`
    // builds the matching set. Construction and the route prefix are delegated to the struct bridges.
    let (singleton_wrappers, singleton_metadata) =
        generate_controller_wrappers(impl_block, struct_name, ControllerScope::Singleton)?;
    let (request_wrappers, request_metadata) =
        generate_controller_wrappers(impl_block, struct_name, ControllerScope::Request)?;

    let toni_routes = generate_toni_routes(struct_name, &singleton_metadata, &request_metadata);

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        #(#singleton_wrappers)*
        #(#request_wrappers)*

        #toni_routes
    })
}

/// Emit the controller's inherent `__toni_routes`, which shadows the `RoutesBridge` empty default.
/// Builds the per-route wrappers for the resolved state — the shared singleton instance, or the
/// dependency map for the per-request rebuild.
fn generate_toni_routes(
    struct_name: &Ident,
    singleton_metadata: &[MetadataInfo],
    request_metadata: &[MetadataInfo],
) -> TokenStream {
    let route_ty = quote! { ::std::sync::Arc<dyn ::toni::traits_helpers::Route> };

    let singleton_creations: Vec<_> = singleton_metadata
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            if metadata.is_static {
                quote! { ::std::sync::Arc::new(#controller_name {}) as #route_ty }
            } else {
                quote! { ::std::sync::Arc::new(#controller_name { instance: inst.clone() }) as #route_ty }
            }
        })
        .collect();

    let request_creations: Vec<_> = request_metadata
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            if metadata.is_static {
                quote! { ::std::sync::Arc::new(#controller_name {}) as #route_ty }
            } else {
                quote! { ::std::sync::Arc::new(#controller_name { dependencies: deps.clone() }) as #route_ty }
            }
        })
        .collect();

    quote! {
        impl #struct_name {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            pub fn __toni_routes(
                state: &::toni::traits_helpers::ControllerInstance,
            ) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Route>> {
                match state {
                    ::toni::traits_helpers::ControllerInstance::Singleton(inst) => {
                        let _ = inst;
                        vec![#(#singleton_creations),*]
                    }
                    ::toni::traits_helpers::ControllerInstance::Request(deps) => {
                        let _ = deps;
                        vec![#(#request_creations),*]
                    }
                }
            }
        }
    }
}

fn generate_controller_wrappers(
    impl_block: &ItemImpl,
    struct_name: &Ident,
    scope: ControllerScope,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>)> {
    let mut wrappers = Vec::new();
    let mut metadata_list = Vec::new();

    let controller_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;

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
                    marker_params,
                    scope,
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

/// Extract #[set_metadata(...)] expressions from method attributes
fn get_metadata_exprs(attrs: &[Attribute]) -> Result<Vec<TokenStream>> {
    let mut metadata_exprs = Vec::new();

    for attr in attrs {
        if attr_is(attr, "set_metadata") {
            let expr: syn::Expr = attr.parse_args()?;
            metadata_exprs.push(quote! { #expr });
        }
    }

    Ok(metadata_exprs)
}

#[allow(clippy::too_many_arguments)]
fn generate_controller_wrapper(
    method: &ImplItemFn,
    struct_name: &Ident,
    http_method_attr: &Attribute,
    controller_enhancers_attr: Vec<(&Ident, &Attribute)>,
    method_enhancers_attr: Vec<(&Ident, &Attribute)>,
    marker_params: Vec<MarkerParam>,
    scope: ControllerScope,
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
    let scope_suffix = match scope {
        ControllerScope::Singleton => "",
        ControllerScope::Request => "Request",
    };
    let controller_name = Ident::new(
        &format!(
            "{}{}Controller{}",
            struct_name,
            capitalize_first(method_name.to_string()),
            scope_suffix
        ),
        method_name.span(),
    );

    let is_static_method = !has_self_receiver(method);

    let enhancer_infos = create_enhancer_infos(controller_enhancers_attr, method_enhancers_attr)?;
    let metadata_exprs = get_metadata_exprs(&method.attrs)?;

    let extractor_params = get_extractor_params(method)?;
    let has_extractors = extractor_params
        .iter()
        .any(|p| !matches!(p.kind, ExtractorKind::HttpRequest | ExtractorKind::Unknown));
    let use_extractors = has_extractors || marker_params.is_empty();

    let (method_call, marker_params_extraction, body_dto_token_stream) = if use_extractors {
        let (extractions, call_args) = generate_extractor_extractions(&extractor_params)?;
        let method_call = if is_static_method {
            generate_extractor_static_method_call(method, struct_name, &call_args)?
        } else {
            generate_extractor_method_call(method, &call_args)?
        };
        (method_call, extractions, None)
    } else {
        let method_call =
            generate_method_call(method, &marker_params, struct_name, is_static_method)?;
        let (extractions, body_dto) = generate_marker_params_extraction(&marker_params)?;
        (method_call, extractions, body_dto)
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

    let wrapper = generate_controller_wrapper_code(
        &controller_name,
        struct_name,
        &route_path,
        &http_method,
        &method_call,
        &enhancer_infos,
        &marker_params_extraction,
        &body_dto_token_stream,
        &metadata_exprs,
        scope,
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

fn generate_marker_params_extraction(
    marker_params: &[MarkerParam],
) -> Result<(Vec<TokenStream>, Option<TokenStream>)> {
    let mut extractions = Vec::new();
    let body_dto_token_stream = None;

    for marker_param in marker_params {
        match marker_param.marker_name.as_str() {
            "body" => extractions.push(extract_body_from_param(marker_param)?),
            "query" => extractions.push(extract_query_from_param(marker_param)?),
            "param" => extractions.push(extract_path_param_from_param(marker_param)?),
            _ => {}
        }
    }

    Ok((extractions, body_dto_token_stream))
}

#[allow(clippy::too_many_arguments)]
fn generate_controller_wrapper_code(
    controller_name: &Ident,
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    method_call: &TokenStream,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
    metadata_exprs: &[TokenStream],
    scope: ControllerScope,
    is_static_method: bool,
    returns_result: bool,
) -> TokenStream {
    match scope {
        ControllerScope::Singleton => generate_singleton_controller_wrapper(
            controller_name,
            struct_name,
            route_path,
            http_method,
            method_call,
            enhancer_infos,
            marker_params_extraction,
            body_dto_token_stream,
            metadata_exprs,
            is_static_method,
            returns_result,
        ),
        ControllerScope::Request => generate_request_controller_wrapper(
            controller_name,
            struct_name,
            route_path,
            http_method,
            method_call,
            enhancer_infos,
            marker_params_extraction,
            body_dto_token_stream,
            metadata_exprs,
            is_static_method,
            returns_result,
        ),
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
    let (pipe_tokens, pipe_instances) = enhancer_vecs(enhancer_infos, "pipes");
    let (error_handler_tokens, error_handler_instances) =
        enhancer_vecs(enhancer_infos, "error_handlers");

    quote! {
        fn enhancers(&self) -> ::toni::traits_helpers::ControllerEnhancers {
            ::toni::traits_helpers::ControllerEnhancers {
                guard_tokens: vec![#(#guard_tokens),*],
                interceptor_tokens: vec![#(#interceptor_tokens),*],
                pipe_tokens: vec![#(#pipe_tokens),*],
                error_handler_tokens: vec![#(#error_handler_tokens),*],
                guards: vec![#(::std::sync::Arc::new(#guard_instances)),*],
                interceptors: vec![#(::std::sync::Arc::new(#interceptor_instances)),*],
                pipes: vec![#(::std::sync::Arc::new(#pipe_instances)),*],
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
    body_dto_token_stream: &Option<TokenStream>,
) -> TokenStream {
    let enhancers = enhancers_method(enhancer_infos);
    let get_path = get_path_method(struct_name, route_path);
    let body_dto_stream = body_dto_token_stream
        .clone()
        .unwrap_or_else(|| quote! { None });

    quote! {
        fn get_method(&self) -> ::toni::http_helpers::HttpMethod {
            ::toni::http_helpers::HttpMethod::from_string(#http_method).unwrap()
        }

        #get_path

        #enhancers

        fn get_route_metadata(&self) -> ::std::sync::Arc<::toni::http_helpers::RouteMetadata> {
            let mut metadata = ::toni::http_helpers::RouteMetadata::new();
            #(metadata.insert(#metadata_exprs);)*
            ::std::sync::Arc::new(metadata)
        }

        fn get_body_dto(&self, _req: &::toni::http_helpers::RequestPart) -> Option<Box<dyn ::toni::traits_helpers::validate::Validatable>> {
            #body_dto_stream
        }
    }
}

// Singleton route wrapper — shares the controller instance built once at startup.
#[allow(clippy::too_many_arguments)]
fn generate_singleton_controller_wrapper(
    controller_name: &Ident,
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    method_call: &TokenStream,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
    metadata_exprs: &[TokenStream],
    is_static_method: bool,
    returns_result: bool,
) -> TokenStream {
    let (struct_fields, instance_downcast) = if is_static_method {
        (quote! {}, quote! {})
    } else {
        (
            quote! {
                instance: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
            },
            quote! {
                let controller = self.instance
                    .downcast_ref::<#struct_name>()
                    .expect("Failed to downcast controller instance");
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
        body_dto_token_stream,
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

                #(#marker_params_extraction)*

                #instance_downcast

                use ::toni::http_helpers::IntoResponse;
                #exec_body
            }

            #common
        }
    }
}

// Request-scoped route wrapper — rebuilds the controller instance per request via the struct bridge.
#[allow(clippy::too_many_arguments)]
fn generate_request_controller_wrapper(
    controller_name: &Ident,
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    method_call: &TokenStream,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
    metadata_exprs: &[TokenStream],
    is_static_method: bool,
    returns_result: bool,
) -> TokenStream {
    let (struct_fields, build_instance) = if is_static_method {
        (quote! {}, quote! {})
    } else {
        (
            quote! {
                dependencies: ::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
                >,
            },
            // Rebuild per request, then fire init/bootstrap on the fresh instance through the
            // lifecycle bridge (no-op when the controller declares no such hooks).
            quote! {
                let controller = #struct_name::__toni_build_from_deps(
                    &self.dependencies,
                    ::toni::ProviderContext::Http(__ctx.clone()),
                ).await;
                {
                    use ::toni::__lifecycle::LifecycleBridge as _;
                    let _ = #struct_name::__toni_lc_on_init(&controller).await;
                    let _ = #struct_name::__toni_lc_on_bootstrap(&controller).await;
                }
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
        body_dto_token_stream,
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

                // Build the instance before the extractors run: building only borrows `_req_parts`
                // (for resolving a request-scoped dependency), while a body extractor may move it.
                #build_instance

                #(#marker_params_extraction)*

                use ::toni::http_helpers::IntoResponse;
                #exec_body
            }

            #common
        }
    }
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

/// Body of the wrapper's `execute` for a user method.
///
/// `Result<T, E>` returns are pattern-matched so the typed `E` flows through the dispatcher as the
/// transport's handler error type — `HttpError` here. `Into::into` calls the `From<E: Error> for
/// HttpError` blanket so the user's domain error is lifted automatically. Plain `T` returns wrap
/// directly in `ExecutionResult::Ok`.
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
