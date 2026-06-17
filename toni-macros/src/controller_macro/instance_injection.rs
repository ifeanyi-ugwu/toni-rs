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
    shared::{
        attr_is,
        lifecycle_hooks::{LifecycleHooks, detect_lifecycle_hooks, strip_lifecycle_attrs},
        metadata_info::MetadataInfo,
        scope_parser::ControllerScope,
    },
    utils::controller_utils::attr_to_string,
};

/// Entry for `#[routes] impl Foo { … }`. The struct (and its DI bridges) is declared separately via
/// `#[controller]`; this never sees the struct's fields.
pub fn generate_routes_system(impl_block: &ItemImpl) -> Result<TokenStream> {
    let struct_name = crate::utils::extracts::extract_impl_self_ident(impl_block)?;
    let struct_name = &struct_name;

    // Lifecycle hooks are detected here (the impl is visible) and the `#[on_*]` attrs stripped so the
    // re-emitted methods are plain; the object's hooks call those methods on the built instance.
    // `#[new]` is left intact so its own macro expands into the `__toni_ctor_build` bridge.
    let lifecycle_hooks = detect_lifecycle_hooks(impl_block);

    let mut impl_def = impl_block.clone();
    for item in impl_def.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            crate::markers_params::remove_marker_controller_fn::remove_marker_in_controller_fn_args(
                method,
            );
        }
    }
    let impl_def = strip_lifecycle_attrs(&impl_def);

    // Scope/elevation is decided at runtime from the struct bridges, so both wrapper sets are always
    // generated; `__toni_routes` (via the factory's chosen state) selects which to build.
    let (singleton_wrappers, singleton_metadata) = generate_controller_wrappers(
        impl_block,
        struct_name,
        ControllerScope::Singleton,
        &lifecycle_hooks,
    )?;
    let (request_wrappers, request_metadata) = generate_controller_wrappers(
        impl_block,
        struct_name,
        ControllerScope::Request,
        &lifecycle_hooks,
    )?;

    let has_instance_method = singleton_metadata.iter().any(|m| !m.is_static)
        || request_metadata.iter().any(|m| !m.is_static);

    let object = generate_controller_object(
        struct_name,
        &singleton_metadata,
        &request_metadata,
        &lifecycle_hooks,
        has_instance_method,
    );
    let factory = generate_factory(struct_name);
    let factory_accessor = generate_controller_factory_accessor(struct_name);

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        #(#singleton_wrappers)*
        #(#request_wrappers)*

        #object
        #factory
        #factory_accessor
    })
}

fn controller_object_ident(struct_name: &Ident) -> Ident {
    Ident::new(
        &format!("{}ControllerObject", struct_name),
        struct_name.span(),
    )
}

/// The single `Controller` per struct: holds the built instance (or the resolved deps for the request
/// path), yields one `Route` per handler method, and carries the lifecycle hooks — fired once.
fn generate_controller_object(
    struct_name: &Ident,
    singleton_metadata: &[MetadataInfo],
    request_metadata: &[MetadataInfo],
    lifecycle_hooks: &LifecycleHooks,
    has_instance_method: bool,
) -> TokenStream {
    let object_name = controller_object_ident(struct_name);
    let struct_token = struct_name.to_string();

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

    let lifecycle_methods = if has_instance_method && lifecycle_hooks.has_any() {
        generate_object_lifecycle_methods(struct_name, lifecycle_hooks)
    } else {
        quote! {}
    };

    quote! {
        pub struct #object_name {
            state: ::toni::traits_helpers::ControllerInstance,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Controller for #object_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn routes(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Route>> {
                match &self.state {
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

            #lifecycle_methods
        }
    }
}

/// Lifecycle-hook overrides that delegate to the singleton instance. The request path fires
/// `on_module_init` / `on_application_bootstrap` per request inside the route handler instead (it has
/// no persistent instance), so these no-op there.
fn generate_object_lifecycle_methods(
    struct_name: &Ident,
    lifecycle_hooks: &LifecycleHooks,
) -> TokenStream {
    let mut methods = Vec::new();

    if let Some(method) = &lifecycle_hooks.on_module_init {
        methods.push(quote! {
            async fn on_module_init(&self) -> ::toni::InitResult {
                if let ::toni::traits_helpers::ControllerInstance::Singleton(inst) = &self.state {
                    if let Some(controller) = inst.downcast_ref::<#struct_name>() {
                        return controller.#method().await;
                    }
                }
                Ok(())
            }
        });
    }
    if let Some(method) = &lifecycle_hooks.on_application_bootstrap {
        methods.push(quote! {
            async fn on_application_bootstrap(&self) -> ::toni::InitResult {
                if let ::toni::traits_helpers::ControllerInstance::Singleton(inst) = &self.state {
                    if let Some(controller) = inst.downcast_ref::<#struct_name>() {
                        return controller.#method().await;
                    }
                }
                Ok(())
            }
        });
    }
    if let Some(method) = &lifecycle_hooks.on_module_destroy {
        methods.push(quote! {
            async fn on_module_destroy(&self) {
                if let ::toni::traits_helpers::ControllerInstance::Singleton(inst) = &self.state {
                    if let Some(controller) = inst.downcast_ref::<#struct_name>() {
                        controller.#method().await;
                    }
                }
            }
        });
    }
    if let Some(method) = &lifecycle_hooks.before_application_shutdown {
        methods.push(quote! {
            async fn before_application_shutdown(&self, signal: Option<String>) {
                if let ::toni::traits_helpers::ControllerInstance::Singleton(inst) = &self.state {
                    if let Some(controller) = inst.downcast_ref::<#struct_name>() {
                        controller.#method(signal).await;
                    }
                }
            }
        });
    }
    if let Some(method) = &lifecycle_hooks.on_application_shutdown {
        methods.push(quote! {
            async fn on_application_shutdown(&self, signal: Option<String>) {
                if let ::toni::traits_helpers::ControllerInstance::Singleton(inst) = &self.state {
                    if let Some(controller) = inst.downcast_ref::<#struct_name>() {
                        controller.#method(signal).await;
                    }
                }
            }
        });
    }

    quote! { #(#methods)* }
}

fn generate_controller_wrappers(
    impl_block: &ItemImpl,
    struct_name: &Ident,
    scope: ControllerScope,
    lifecycle_hooks: &LifecycleHooks,
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
                    lifecycle_hooks,
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
    })
}

fn get_enhancers_attr(attrs: &[syn::Attribute]) -> Result<HashMap<&Ident, &Attribute>> {
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
    controller_enhancers_attr: HashMap<&Ident, &Attribute>,
    method_enhancers_attr: HashMap<&Ident, &Attribute>,
    marker_params: Vec<MarkerParam>,
    scope: ControllerScope,
    lifecycle_hooks: &LifecycleHooks,
) -> Result<(TokenStream, MetadataInfo)> {
    let http_method = attr_to_string(http_method_attr)
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?;

    // Sub-path only; the controller's prefix is joined at runtime via `__toni_prefix`.
    let route_path = http_method_attr
        .parse_args::<LitStr>()
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?
        .value();

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
        lifecycle_hooks,
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
    lifecycle_hooks: &LifecycleHooks,
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
            lifecycle_hooks,
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
                __req: ::toni::http_helpers::HttpRequest,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::toni::http_helpers::HttpResponse,
                ::toni::errors::HttpError,
            > {
                let (_req_parts, _req_body) = __req.0.into_parts();

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
    lifecycle_hooks: &LifecycleHooks,
    returns_result: bool,
) -> TokenStream {
    let (struct_fields, build_instance) = if is_static_method {
        (quote! {}, quote! {})
    } else {
        let init_call = lifecycle_hooks
            .on_module_init
            .as_ref()
            .map(|m| quote! { controller.#m().await; });
        let bootstrap_call = lifecycle_hooks
            .on_application_bootstrap
            .as_ref()
            .map(|m| quote! { controller.#m().await; });
        (
            quote! {
                dependencies: ::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
                >,
            },
            quote! {
                let controller = #struct_name::__toni_build_from_deps(
                    &self.dependencies,
                    ::std::option::Option::Some(&_req_parts),
                ).await;
                #init_call
                #bootstrap_call
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
                __req: ::toni::http_helpers::HttpRequest,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::toni::http_helpers::HttpResponse,
                ::toni::errors::HttpError,
            > {
                let (_req_parts, _req_body) = __req.0.into_parts();

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

fn generate_controller_factory_accessor(struct_name: &Ident) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ControllerFactory", struct_name),
        struct_name.span(),
    );
    quote! {
        impl #struct_name {
            #[doc(hidden)]
            pub fn __toni_controller_factory() -> impl ::toni::traits_helpers::ControllerFactory {
                #factory_name
            }
        }
    }
}

/// One factory drives both scopes: it asks the struct bridges for the dependency tokens and declared
/// scope at runtime, elevates an (implicit/explicit) singleton to request scope when any dependency is
/// request-scoped, and otherwise builds the instance once via `__toni_build_from_deps`.
fn generate_factory(struct_name: &Ident) -> TokenStream {
    let factory_name = Ident::new(
        &format!("{}ControllerFactory", struct_name),
        struct_name.span(),
    );
    let object_name = controller_object_ident(struct_name);
    let struct_token = struct_name.to_string();

    quote! {
        pub struct #factory_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ControllerFactory for #factory_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                <#struct_name>::__toni_dependencies()
            }

            async fn build(
                &self,
                dependencies: ::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>
                >,
            ) -> ::std::sync::Arc<dyn ::toni::traits_helpers::Controller> {
                let force_request = <#struct_name>::__toni_is_request_scoped();

                let mut request_deps: Vec<String> = Vec::new();
                for __token in <#struct_name>::__toni_dependencies() {
                    if let Some(__provider) = dependencies.get(&__token) {
                        if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
                            request_deps.push(__token);
                        }
                    }
                }

                if !force_request && !request_deps.is_empty() {
                    ::toni::tracing::warn!(
                        controller = #struct_token,
                        request_scoped_deps = ?request_deps,
                        "Controller automatically elevated to request scope due to request-scoped \
                         providers. Silence this by declaring #[controller(scope = \"request\")]."
                    );
                }

                let state = if force_request || !request_deps.is_empty() {
                    ::toni::traits_helpers::ControllerInstance::Request(dependencies)
                } else {
                    let controller_instance: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync> =
                        ::std::sync::Arc::new(
                            <#struct_name>::__toni_build_from_deps(
                                &dependencies,
                                ::std::option::Option::None,
                            ).await,
                        );
                    ::toni::traits_helpers::ControllerInstance::Singleton(controller_instance)
                };

                ::std::sync::Arc::new(#object_name { state })
            }
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
