//! Extractor parameter detection and code generation
//!
//! Detects extractor types like `Path<T>`, `Query<T>`, `Json<T>` and
//! `Validated<T>` to decide which of them reads the body. Extraction itself goes through
//! `FromContext` whatever the type is.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, Ident, ImplItemFn, Result, Type};

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
    /// `Validated<T>` — reads whatever `T` reads, so it takes the body only when
    /// the extractor it wraps does.
    Validated {
        /// The kind of the extractor being validated
        inner_kind: Box<ExtractorKind>,
    },
    /// HttpRequest (not an extractor, just passed through — body-consuming)
    HttpRequest,
    /// Request extractor — parts-only
    Request,
    /// Extensions extractor (the per-message bag) — parts-only
    Extensions,
    /// `&HttpContext` — the handler context itself, forwarded rather than extracted
    Context,
    /// Option<T> wrapped extractor (optional extraction)
    Optional {
        /// The inner extractor kind
        inner_kind: Box<ExtractorKind>,
        /// The inner type T from Option<T>
        inner_type: Type,
    },
    /// A type the macro does not recognise — extracted through `FromContext`
    /// like any other, so a custom extractor needs no special handling here.
    Unknown,
}

impl ExtractorKind {
    /// Whether this extractor is a *named* body consumer — the ones that
    /// actually receive the request body.
    ///
    /// `Unknown` is excluded even though it is generated on the body-consuming
    /// path: several `Unknown` parameters are legal, because each is handed an
    /// empty body so custom parts-only extractors can coexist. Only one of
    /// these can be served.
    fn takes_the_body(&self) -> bool {
        match self {
            ExtractorKind::Json
            | ExtractorKind::Body
            | ExtractorKind::Bytes
            | ExtractorKind::BodyStream
            | ExtractorKind::HttpRequest => true,
            ExtractorKind::Optional { inner_kind, .. }
            | ExtractorKind::Validated { inner_kind } => inner_kind.takes_the_body(),
            _ => false,
        }
    }
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
pub fn get_extractor_params(method: &ImplItemFn) -> Result<Vec<ExtractorParam>> {
    let mut params = Vec::new();

    for input in method.sig.inputs.iter() {
        if let FnArg::Typed(pat_type) = input {
            // Skip parameters with marker attributes (#[body], #[query], #[param])
            if !pat_type.attrs.is_empty()
                && pat_type.attrs[0].path().segments.last().is_some_and(|seg| {
                    crate::markers_params::remove_marker_controller_fn::is_marker(&seg.ident)
                })
            {
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

    reject_second_body_extractor(&params)?;

    Ok(params)
}

/// A request body is read once — it may be a stream, so there is nothing to
/// hand a second reader.
///
/// The generated code moves the body into the first consumer, so a second one
/// is already rejected, but as a use-of-moved-value pointing at `#[routes]` and
/// suggesting `ref #[routes]`. Naming the two parameters is the difference
/// between a diagnosis and a puzzle.
fn reject_second_body_extractor(params: &[ExtractorParam]) -> Result<()> {
    let mut consumers = params.iter().filter(|p| p.kind.takes_the_body());

    let (Some(first), Some(second)) = (consumers.next(), consumers.next()) else {
        return Ok(());
    };

    Err(syn::Error::new_spanned(
        &second.param_type,
        format!(
            "`{}` and `{}` both read the request body, and it can only be read once.\n\
             Keep one of them. If you need more than one view of the body, take \
             `Bytes` (or `HttpRequest`) and parse it yourself.",
            first.param_name, second.param_name,
        ),
    ))
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
                return ExtractorKind::Optional {
                    inner_kind: Box::new(detect_extractor_kind(&inner_type)),
                    inner_type,
                };
            }

            if type_name == "Validated" {
                let Some(inner_type) = first_type_argument(segment) else {
                    return ExtractorKind::Unknown;
                };
                return ExtractorKind::Validated {
                    inner_kind: Box::new(detect_extractor_kind(&inner_type)),
                };
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
            ExtractorKind::Optional { inner_type, .. } => quote! {
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

    fn handler(sig: syn::ImplItemFn) -> Result<Vec<ExtractorParam>> {
        get_extractor_params(&sig)
    }

    #[test]
    fn one_body_extractor_beside_parts_extractors_is_fine() {
        let ok = handler(parse_quote! {
            fn h(&self, Path(id): Path<u32>, q: Query<Filter>, dto: Json<Dto>) {}
        });
        assert!(ok.is_ok());
    }

    #[test]
    fn two_body_extractors_are_rejected_by_name() {
        let msg = match handler(parse_quote! {
            fn h(&self, dto: Json<Dto>, raw: Bytes) {}
        }) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a body cannot be read twice"),
        };
        assert!(msg.contains("dto"), "names the first reader: {msg}");
        assert!(msg.contains("raw"), "names the second reader: {msg}");
    }

    /// `Option<Json<T>>` still reads the body, so it counts.
    #[test]
    fn an_optional_body_extractor_still_counts() {
        assert!(
            handler(parse_quote! {
                fn h(&self, dto: Option<Json<Dto>>, raw: Bytes) {}
            })
            .is_err()
        );
    }

    /// `HttpRequest` hands over the whole request, body included.
    #[test]
    fn the_raw_request_counts_as_a_reader() {
        assert!(
            handler(parse_quote! {
                fn h(&self, req: HttpRequest, raw: Bytes) {}
            })
            .is_err()
        );
    }

    /// Unrecognised types are generated on the body path but handed an empty
    /// body, so several of them coexist — that is how custom parts-only
    /// extractors work.
    #[test]
    fn several_custom_extractors_coexist() {
        assert!(
            handler(parse_quote! {
                fn h(&self, a: MyHeader, b: MyOtherHeader) {}
            })
            .is_ok()
        );
    }
}
