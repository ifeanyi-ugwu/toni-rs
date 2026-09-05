//! Extractor parameter detection and code generation
//!
//! Extraction goes through `FromContext` whatever the type is. The kinds here
//! decide the few parameters whose code differs — the context, which is not
//! extracted, and `Option<T>`, which answers `None` rather than a 400 — while
//! which parameter reads the body is read off `FromContext::CONSUMES` rather
//! than off any name.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, Ident, ImplItemFn, Result, Type, parse_quote};

/// Check if a method has a `self` receiver (i.e., is an instance method)
pub fn has_self_receiver(method: &ImplItemFn) -> bool {
    method
        .sig
        .inputs
        .iter()
        .any(|arg| matches!(arg, FnArg::Receiver(_)))
}

/// Information about an extractor parameter
#[derive(Clone)]
pub struct ExtractorParam {
    /// The parameter name (e.g., `id` from `Path(id): Path<i32>`)
    pub param_name: Ident,
    /// The full type (e.g., `Path<i32>`)
    pub param_type: Type,
    /// The extractor kind
    pub kind: ExtractorKind,
}

/// The kind of extractor
#[derive(Clone)]
pub enum ExtractorKind {
    /// Path<T> extractor — parts-only
    Path,
    /// Query<T> extractor — parts-only
    Query,
    /// Json<T> extractor — body-consuming
    Json,
    /// Body<T> extractor (auto-detects content type) — body-consuming
    Body,
    /// Bytes extractor (raw binary data) — body-consuming
    Bytes,
    /// BodyStream extractor (streaming body) — body-consuming
    BodyStream,
    /// `Validated<T>` — extracted through `FromContext` like the type it wraps.
    Validated,
    /// HttpRequest (not an extractor, just passed through — body-consuming)
    HttpRequest,
    /// Request extractor — parts-only
    Request,
    /// Extensions extractor (the per-message bag) — parts-only
    Extensions,
    /// `&HttpContext` — the handler context itself, forwarded rather than extracted
    Context,
    /// `Option<T>` — extracted as `T`, answering `None` where that fails.
    Optional {
        /// The `T` in `Option<T>`
        inner_type: Type,
    },
    /// A type the macro does not recognise — extracted through `FromContext`
    /// like any other, so a custom extractor needs no special handling here.
    Unknown,
}

/// Recursively extract parameter name from potentially nested patterns
/// Handles: `dto`, `Json(dto)`, `Validated(Json(dto))`, etc.
///
/// The generated code binds this name to the *whole* extracted value; the
/// destructuring happens in the user's own signature.
pub(crate) fn extract_param_name(pat: &syn::Pat) -> Option<Ident> {
    match pat {
        syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.clone()),
        syn::Pat::TupleStruct(tuple_struct) => {
            if let Some(inner) = tuple_struct.elems.first() {
                extract_param_name(inner)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract extractor parameters from a method signature
pub fn get_extractor_params(
    method: &ImplItemFn,
) -> Result<(Vec<ExtractorParam>, Vec<(Ident, Type)>)> {
    let mut params = Vec::new();
    let mut body_markers: Vec<(Ident, Type)> = Vec::new();

    for input in method.sig.inputs.iter() {
        if let FnArg::Typed(pat_type) = input {
            // A marker names its own source, and its extraction is the marker
            // machinery's rather than this one's. `#[body]` reads through
            // `Body<T>`, so the one-body assertion counts it as that.
            let marker = pat_type.attrs.first().and_then(|attr| {
                attr.path().segments.last().filter(|seg| {
                    crate::markers_params::remove_marker_controller_fn::is_marker(&seg.ident)
                })
            });
            if let Some(marker) = marker {
                if marker.ident == "body"
                    && let Some(param_name) = extract_param_name(&pat_type.pat)
                {
                    let inner = &*pat_type.ty;
                    body_markers.push((param_name, parse_quote!(::toni::extractors::Body<#inner>)));
                }
                continue;
            }

            let param_name = extract_param_name(&pat_type.pat);
            let param_name = match param_name {
                Some(name) => name,
                None => continue,
            };

            if param_name == "self" {
                continue;
            }

            let param_type = (*pat_type.ty).clone();
            let kind = detect_extractor_kind(&param_type);

            params.push(ExtractorParam {
                param_name,
                param_type,
                kind,
            });
        }
    }

    Ok((params, body_markers))
}

/// A request body is read once — it may be a stream, so there is nothing to
/// hand a second reader.
///
/// Which parameter reads it comes off the types rather than their names: each
/// contributes `<Ty as FromContext<HttpContext>>::CONSUMES`, so an alias, a
/// wrapper and a custom extractor that declares itself all count the same. One
/// assertion per pair, because a sum could say only that two of several read
/// the body while a pair names both.
pub fn one_body_assertion(
    params: &[ExtractorParam],
    body_markers: &[(Ident, Type)],
) -> TokenStream {
    let counted: Vec<(&Ident, &Type)> = params
        .iter()
        // `&HttpContext` is not extracted, and a reference has no impl to read.
        .filter(|p| !matches!(p.kind, ExtractorKind::Context))
        .map(|p| (&p.param_name, &p.param_type))
        .chain(body_markers.iter().map(|(name, ty)| (name, ty)))
        .collect();

    let mut assertions = Vec::new();
    for (i, (first_name, first_ty)) in counted.iter().enumerate() {
        for (second_name, second_ty) in counted.iter().skip(i + 1) {
            let message = format!(
                "`{first_name}` and `{second_name}` both read the request body, and it can only \
                 be read once.\nKeep one of them. If you need more than one view of the body, \
                 take `Bytes` (or `HttpRequest`) and parse it yourself.",
            );
            assertions.push(quote! {
                const _: () = {
                    assert!(
                        !(<#first_ty as ::toni::extractors::FromContext<
                            ::toni::context::HttpContext,
                        >>::CONSUMES
                            && <#second_ty as ::toni::extractors::FromContext<
                                ::toni::context::HttpContext,
                            >>::CONSUMES),
                        #message
                    );
                };
            });
        }
    }

    quote! { #(#assertions)* }
}

/// The `T` in `Wrapper<T>`, when the type is written with one.
fn first_type_argument(segment: &syn::PathSegment) -> Option<Type> {
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first() {
        Some(syn::GenericArgument::Type(inner)) => Some(inner.clone()),
        _ => None,
    }
}

/// Detect what kind of extractor a type is
fn detect_extractor_kind(ty: &Type) -> ExtractorKind {
    // `&HttpContext` is the only reference a handler may take — every extractor
    // owns what it produces.
    if let Type::Reference(type_ref) = ty {
        if let Type::Path(inner) = &*type_ref.elem
            && inner
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "HttpContext")
        {
            return ExtractorKind::Context;
        }
        return ExtractorKind::Unknown;
    }

    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();

            // Both wrappers defer to what they wrap, so an unreadable inner type
            // leaves nothing to defer to: `Unknown`, which is not counted as a
            // body consumer.
            if type_name == "Option" {
                let Some(inner_type) = first_type_argument(segment) else {
                    return ExtractorKind::Unknown;
                };
                return ExtractorKind::Optional { inner_type };
            }

            if type_name == "Validated" {
                return ExtractorKind::Validated;
            }

            return match type_name.as_str() {
                "Path" => ExtractorKind::Path,
                "Query" => ExtractorKind::Query,
                "Json" => ExtractorKind::Json,
                "Body" => ExtractorKind::Body,
                "Bytes" => ExtractorKind::Bytes,
                "BodyStream" => ExtractorKind::BodyStream,
                "Multipart" => ExtractorKind::Body,
                "HttpRequest" => ExtractorKind::HttpRequest,
                "Request" => ExtractorKind::Request,
                "Extensions" => ExtractorKind::Extensions,
                _ => ExtractorKind::Unknown,
            };
        }
    }
    ExtractorKind::Unknown
}

/// Generate extraction code for extractor parameters.
///
/// Every parameter is a `FromContext<HttpContext>` read from the context, in
/// signature order. Nothing is moved out of a shared local, so the order carries
/// no constraint of its own: whichever extractor reads the body finds it, and
/// anything looking afterwards is told it has gone.
pub fn generate_extractor_extractions(
    params: &[ExtractorParam],
) -> Result<(Vec<TokenStream>, Vec<TokenStream>)> {
    let mut extractions = Vec::new();

    for param in params {
        let param_name = &param.param_name;
        let param_type = &param.param_type;

        let extraction = match &param.kind {
            // Not extracted — the dispatcher owns it, and it is reborrowed at the
            // call itself so it does not hold a mutable borrow across the
            // extractions that follow.
            ExtractorKind::Context => continue,

            // The whole request, body included, under the same single-use rule as
            // any other body reader.
            ExtractorKind::HttpRequest => {
                let failure = extraction_failed(quote! {
                    "the request body was already read by another extractor on this handler"
                });
                quote! {
                    let #param_name = match __ctx.take_request() {
                        ::std::option::Option::Some(__req) => __req,
                        ::std::option::Option::None => { #failure }
                    };
                }
            }

            // `None` on failure instead of a 400.
            ExtractorKind::Optional { inner_type } => quote! {
                let #param_name = <#inner_type as ::toni::extractors::FromContext<
                    ::toni::context::HttpContext,
                >>::extract(__ctx).await.ok();
            },

            _ => {
                let failure = extraction_failed(quote! { __e.to_string() });
                quote! {
                    let #param_name = match <#param_type as ::toni::extractors::FromContext<
                        ::toni::context::HttpContext,
                    >>::extract(__ctx).await {
                        ::std::result::Result::Ok(__value) => __value,
                        ::std::result::Result::Err(__e) => { #failure }
                    };
                }
            }
        };
        extractions.push(extraction);
    }

    // Call args follow the signature. The context is reborrowed here rather than
    // bound above, so every extraction has had exclusive access in turn.
    let call_args: Vec<TokenStream> = params
        .iter()
        .map(|p| {
            let name = &p.param_name;
            match &p.kind {
                ExtractorKind::Context => quote! { &*__ctx },
                _ => quote! { #name },
            }
        })
        .collect();

    Ok((extractions, call_args))
}

/// The 400 an extractor returns when it cannot produce its value.
fn extraction_failed(details: TokenStream) -> TokenStream {
    quote! {
        let __error_body = ::toni::serde_json::json!({
            "error": "Extraction failed",
            "details": #details,
        });
        return ::toni::http_helpers::ExecutionResult::Ok(::toni::http_helpers::HttpResponse {
            body: Some(::toni::http_helpers::Body::json(__error_body)),
            status: 400,
            headers: vec![],
        });
    }
}

/// Generate the method call with extracted parameters
pub fn generate_extractor_method_call(
    method: &ImplItemFn,
    call_args: &[TokenStream],
) -> Result<TokenStream> {
    let method_name = &method.sig.ident;
    let is_async = method.sig.asyncness.is_some();

    let call = quote! { controller.#method_name(#(#call_args),*) };

    Ok(if is_async {
        quote! { #call.await }
    } else {
        call
    })
}

/// Generate the method call for static methods (no self receiver)
pub fn generate_extractor_static_method_call(
    method: &ImplItemFn,
    struct_name: &Ident,
    call_args: &[TokenStream],
) -> Result<TokenStream> {
    let method_name = &method.sig.ident;
    let is_async = method.sig.asyncness.is_some();

    let call = quote! { #struct_name::#method_name(#(#call_args),*) };

    Ok(if is_async {
        quote! { #call.await }
    } else {
        call
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn assertion_for(sig: syn::ImplItemFn) -> String {
        let (params, markers) = get_extractor_params(&sig).expect("params parse");
        one_body_assertion(&params, &markers).to_string()
    }

    /// The pair is asserted whatever the types turn out to be — the assertion
    /// is what carries the question to the compiler, and `CONSUMES` answers it.
    #[test]
    fn every_pair_of_extractors_is_asserted() {
        let emitted = assertion_for(parse_quote! {
            fn h(&self, Path(id): Path<u32>, q: Query<Filter>, dto: Json<Dto>) {}
        });
        for pair in [("id", "q"), ("id", "dto"), ("q", "dto")] {
            assert!(
                emitted.contains(&format!("`{}` and `{}`", pair.0, pair.1)),
                "{pair:?} is not asserted: {emitted}"
            );
        }
    }

    #[test]
    fn an_assertion_names_both_parameters() {
        let emitted = assertion_for(parse_quote! {
            fn h(&self, dto: Json<Dto>, raw: Bytes) {}
        });
        assert!(emitted.contains("`dto` and `raw`"), "{emitted}");
        assert!(emitted.contains("Json"), "reads the first type: {emitted}");
        assert!(
            emitted.contains("Bytes"),
            "reads the second type: {emitted}"
        );
    }

    /// A wrapper contributes the type as written; `Option<Json<T>>` forwards
    /// `CONSUMES` through its own impl rather than through anything here.
    #[test]
    fn a_wrapper_contributes_the_written_type() {
        let emitted = assertion_for(parse_quote! {
            fn h(&self, dto: Option<Json<Dto>>, raw: Bytes) {}
        });
        assert!(emitted.contains("Option"), "{emitted}");
        assert!(emitted.contains("`dto` and `raw`"), "{emitted}");
    }

    /// A `#[body]` marker reads through `Body<T>`, and is counted as that.
    #[test]
    fn a_body_marker_is_counted_as_its_extractor() {
        let emitted = assertion_for(parse_quote! {
            fn h(&self, dto: Json<Dto>, #[body] raw: String) {}
        });
        assert!(emitted.contains("`dto` and `raw`"), "{emitted}");
        assert!(
            emitted.contains("Body") && emitted.contains("String"),
            "the marker is counted as `Body<String>`: {emitted}"
        );
    }

    /// The context is not extracted, so there is no impl to read it through.
    #[test]
    fn the_context_is_not_counted() {
        let emitted = assertion_for(parse_quote! {
            fn h(&self, ctx: &HttpContext, dto: Json<Dto>) {}
        });
        assert!(
            emitted.is_empty(),
            "a lone extractor needs no pair: {emitted}"
        );
    }

    #[test]
    fn a_lone_parameter_is_asserted_against_nothing() {
        assert!(assertion_for(parse_quote! { fn h(&self, dto: Json<Dto>) {} }).is_empty());
    }
}
