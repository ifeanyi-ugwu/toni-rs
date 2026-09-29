//! `#[routes]` impl-side controller codegen.
//!
//! `#[controller("/p")]` on the struct ([controller_attr]) emits the DI bridges
//! (`__ulo_build_from_deps` / `__ulo_dependencies` / `__ulo_prefix` / `__ulo_is_execution_scoped`).
//! `#[routes]` on the impl — this module — scans the handler methods and emits one `Route` wrapper
//! per handler method plus the shadowing `__ulo_dispatch` that answers `Dispatch::Http` with
//! them, built around the controller's `DispatchSource`. Each wrapper resolves its instance
//! through the source at call time, so one wrapper set serves both the shared-singleton and the
//! built-per-request controller.
//!
//! The two sides never see each other's item; they meet at the concrete type through the inherent
//! bridge fns. A missing `#[controller]` struct surfaces as "no associated function `__ulo_build_from_deps`".

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use std::collections::HashMap;
use syn::{Attribute, Error, Ident, ImplItemFn, ItemImpl, LitStr, Result, spanned::Spanned};

use crate::{
    controller_macro::extractor_params::{
        ExtractorKind, generate_extractor_extractions, generate_extractor_method_call,
        generate_extractor_static_method_call, get_extractor_params, has_self_receiver,
        one_body_assertion,
    },
    enhancer::enhancer::{EnhancerInfo, create_enhancer_infos, enhancer_entries},
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
    // the `__ULO_ONE_NEW_PER_TYPE` / `__ulo_lc_*` bridges that `#[controller]`'s factory and object
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
            capture_nothing_in_sse_return(method);
        }
    }

    // One wrapper set serves both scopes: each wrapper holds the controller's `DispatchSource`
    // and resolves its instance at call time. Construction and the route prefix are delegated to
    // the struct bridges.
    let (wrappers, metadata) = generate_controller_wrappers(impl_block, struct_name)?;

    let ulo_dispatch = generate_ulo_dispatch(struct_name, &metadata);

    Ok(quote! {
        #[allow(dead_code)]
        #impl_def

        #(#wrappers)*

        #ulo_dispatch
    })
}

/// Emit the controller's inherent `__ulo_dispatch`, which shadows the `DispatchBridge` default
/// and names HTTP: one route wrapper per handler, each holding a clone of the controller's source.
fn generate_ulo_dispatch(struct_name: &Ident, metadata: &[MetadataInfo]) -> TokenStream {
    let route_ty = quote! { ::std::sync::Arc<dyn ::ulo::http::Route> };

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
            pub fn __ulo_dispatch(
                source: &::ulo::__enhancer::DispatchSource<#struct_name>,
            ) -> ::ulo::dispatch::Targets {
                let _ = source;
                ::ulo::dispatch::Targets::Http(vec![#(#creations),*])
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

    // Sub-path only; the controller's prefix is joined at runtime via `__ulo_prefix`.
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

    let (extractor_params, body_markers) = get_extractor_params(method)?;
    let one_body = one_body_assertion(&extractor_params, &body_markers);
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

    // One call covers a stream, a stream of `Result`, and a `Result` of either; the return type
    // is not read. Spanned at that return type: a value none of the three accepts is reported
    // there rather than at `#[routes]`.
    let method_call = if is_sse {
        let at = method.sig.output.span();
        quote_spanned! { at => ::ulo::__http::into_sse(#method_call) }
    } else {
        method_call
    };

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
        is_sse,
    );

    Ok((
        quote! {
            #one_body
            #wrapper
        },
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
    is_sse: bool,
) -> TokenStream {
    let (struct_fields, resolve_instance) = if is_static_method {
        (quote! {}, quote! {})
    } else {
        (
            quote! {
                source: ::ulo::__enhancer::DispatchSource<#struct_name>,
            },
            // Resolve the instance before the extractors run: a per-call build reads
            // execution-scoped dependencies through the context, while a body extractor
            // may move the request out of it. A per-call build whose startup hook fails
            // answers the call with that failure.
            quote! {
                let controller = match self.source
                    .resolve(::ulo::di::Execution::Http(__ctx.clone()))
                    .await
                {
                    ::std::result::Result::Ok(__controller) => __controller,
                    ::std::result::Result::Err(__failed) => {
                        return ::ulo::dispatch::ExecutionResult::Err(
                            ::ulo::http::HttpError::from(__failed),
                        );
                    }
                };
            },
        )
    };

    let exec_body = exec_body_for(method_call);
    let common = route_common_methods(
        struct_name,
        route_path,
        http_method,
        enhancer_infos,
        metadata_exprs,
        is_sse,
    );

    quote! {
        struct #controller_name {
            #struct_fields
        }

        #[::ulo::async_trait]
        impl ::ulo::http::Route for #controller_name {
            async fn execute(
                &self,
                __ctx: &::ulo::http::HttpContext,
            ) -> ::ulo::dispatch::ExecutionResult<
                ::ulo::http::HttpResponse,
                ::ulo::http::HttpError,
            > {
                // Cloned, not borrowed: building an execution-scoped dependency holds
                // the parts across an await, and the extractions below need the
                // context back exclusively.
                let _req_parts = __ctx.request().clone();

                #resolve_instance

                #(#marker_params_extraction)*

                #exec_body
            }

            #common
        }
    }
}

fn enhancers_method(enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>) -> TokenStream {
    let transport = quote! { ::ulo::dispatch::Http };
    let guards = enhancer_entries(enhancer_infos, "guards", &transport);
    let interceptors = enhancer_entries(enhancer_infos, "interceptors", &transport);
    let error_handlers = enhancer_entries(enhancer_infos, "error_handlers", &transport);

    quote! {
        fn enhancers(&self) -> ::ulo::http::RouteEnhancers {
            ::ulo::http::RouteEnhancers {
                guards: vec![#(#guards),*],
                interceptors: vec![#(#interceptors),*],
                error_handlers: vec![#(#error_handlers),*],
            }
        }
    }
}

/// `path` joins the controller's runtime prefix (`__ulo_prefix`) with this route's sub-path.
fn get_path_method(struct_name: &Ident, route_path: &str) -> TokenStream {
    quote! {
        fn path(&self) -> String {
            ::ulo::http::join_route(#struct_name::__ulo_prefix(), #route_path)
        }
    }
}

fn route_common_methods(
    struct_name: &Ident,
    route_path: &str,
    http_method: &str,
    enhancer_infos: &HashMap<String, Vec<EnhancerInfo>>,
    metadata_exprs: &[TokenStream],
    is_sse: bool,
) -> TokenStream {
    let enhancers = enhancers_method(enhancer_infos);
    let path = get_path_method(struct_name, route_path);
    // Only emitted for `#[sse]`; the trait's default answers for every other route.
    let streams = is_sse.then(|| {
        quote! {
            fn streams(&self) -> bool {
                true
            }
        }
    });
    quote! {
        #streams

        fn method(&self) -> ::ulo::http::HttpMethod {
            ::ulo::http::HttpMethod::from_string(#http_method).unwrap()
        }

        #path

        #enhancers

        fn metadata(&self) -> ::std::sync::Arc<::ulo::context::Metadata> {
            let mut metadata = ::ulo::context::Metadata::new();
            #(metadata.insert(#metadata_exprs);)*
            ::std::sync::Arc::new(metadata)
        }

    }
}

/// One shape for every handler.
///
/// No return type is read to decide the conversion. What the value converts to is decided by
/// `IntoOutput<Http>`, which a
/// `Result` satisfies through its own impl, so a handler's `Err` reaches the error side whatever
/// its return type is spelled as.
fn exec_body_for(method_call: &TokenStream) -> TokenStream {
    quote! {
        match ::ulo::dispatch::IntoOutput::<::ulo::__enhancer::Http>::into_output(#method_call) {
            ::std::result::Result::Ok(__out) => ::ulo::dispatch::ExecutionResult::Ok(__out),
            ::std::result::Result::Err(__e) => ::ulo::dispatch::ExecutionResult::Err(__e),
        }
    }
}

/// Appends `+ use<>` to every `impl Trait` in an `#[sse]` handler's return type.
///
/// Rust 2024 has an `impl Trait` in return position capture every lifetime in scope, `&self`
/// among them, so the stream a handler returns is not `'static`. The route wrapper needs it to
/// be: the stream outlives the call, and `Body::stream` says so in its bounds. Without the
/// capture bound the expansion fails with E0716 pointed at `#[routes]`, naming neither the
/// handler nor the remedy.
///
/// Capturing nothing is always right here. A stream that borrowed from the handler could not be
/// returned from the route whatever the author wrote, so the bound removes an error rather than
/// a capability — and the error that remains lands on the handler's own signature.
///
/// Every opaque type in the return position is reached, not the outermost one: `#[sse]` also
/// takes `Result<impl Stream<..>, E>`, where the stream is nested. Walking the type is what
/// finds it, rather than matching the return type against a shape.
///
/// Left alone where the author has said something the bound would contradict: a method with a
/// type or const parameter, which `use<>` would have to name, and an opaque type that already
/// carries a `use<..>` or names a lifetime. A lifetime parameter on the method is not a reason
/// to stop — `use<>` need not name lifetimes.
fn capture_nothing_in_sse_return(method: &mut ImplItemFn) {
    if !method.attrs.iter().any(|attr| attr_is(attr, "sse")) {
        return;
    }
    let names_a_param = method.sig.generics.params.iter().any(|param| {
        matches!(
            param,
            syn::GenericParam::Type(_) | syn::GenericParam::Const(_)
        )
    });
    if names_a_param {
        return;
    }
    syn::visit_mut::VisitMut::visit_return_type_mut(&mut CaptureNothing, &mut method.sig.output);
}

struct CaptureNothing;

impl syn::visit_mut::VisitMut for CaptureNothing {
    fn visit_type_impl_trait_mut(&mut self, opaque: &mut syn::TypeImplTrait) {
        syn::visit_mut::visit_type_impl_trait_mut(self, opaque);
        // A lifetime bound is the author capturing on purpose. Appending `use<>` beside it does
        // not make such a handler compile — it never did — and turns one error into two.
        let author_said_something = opaque.bounds.iter().any(|bound| {
            matches!(
                bound,
                syn::TypeParamBound::PreciseCapture(_) | syn::TypeParamBound::Lifetime(_)
            )
        });
        if !author_said_something {
            opaque.bounds.push(syn::parse_quote!(use<>));
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
