//! Extractor parameter detection and code generation
//!
//! Detects extractor types like Path<T>, Query<T>, Json<T>, Validated<T>
//! and generates FromRequestParts or FromRequest extraction code accordingly.

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
    /// Validated<T> extractor — body-consuming
    Validated,
    /// HttpRequest (not an extractor, just passed through — body-consuming)
    HttpRequest,
    /// Request extractor — parts-only
    Request,
    /// Extensions extractor (the per-message bag) — parts-only
    Extensions,
    /// `&mut HttpContext` — the handler context itself, forwarded rather than extracted
    Context,
    /// Option<T> wrapped extractor (optional extraction)
    Optional {
        /// The inner extractor kind
        inner_kind: Box<ExtractorKind>,
        /// The inner type T from Option<T>
        inner_type: Type,
    },
    /// Unknown type — treated as body-consuming (FromRequest)
    Unknown,
}

impl ExtractorKind {
    /// Whether this extractor consumes the body (and therefore `req` itself).
    /// Body-consuming extractors must be generated after all parts extractors.
    pub fn is_body_consuming(&self) -> bool {
        matches!(
            self,
            ExtractorKind::Json
                | ExtractorKind::Body
                | ExtractorKind::Bytes
                | ExtractorKind::BodyStream
                | ExtractorKind::Validated
                | ExtractorKind::HttpRequest
                | ExtractorKind::Unknown
        )
    }
}

/// Recursively extract parameter name from potentially nested patterns
/// Handles: `dto`, `Json(dto)`, `Validated(Json(dto))`, etc.
fn extract_param_name(pat: &syn::Pat) -> Option<Ident> {
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

    Ok(params)
}

/// Detect what kind of extractor a type is
fn detect_extractor_kind(ty: &Type) -> ExtractorKind {
    // `&mut HttpContext` is the only reference a handler may take — every
    // extractor owns what it produces. Exclusive rather than shared because the
    // wrapper's future must be `Send` and `HttpContext` is not `Sync`.
    if let Type::Reference(type_ref) = ty {
        if let Type::Path(inner) = &*type_ref.elem {
            if inner
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "HttpContext")
            {
                return ExtractorKind::Context;
            }
        }
        return ExtractorKind::Unknown;
    }

    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();

            if type_name == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                        let inner_kind = detect_extractor_kind(inner_type);
                        return ExtractorKind::Optional {
                            inner_kind: Box::new(inner_kind),
                            inner_type: inner_type.clone(),
                        };
                    }
                }
                // Can't extract the inner type — treat as Unknown.
                return ExtractorKind::Unknown;
            }

            return match type_name.as_str() {
                "Path" => ExtractorKind::Path,
                "Query" => ExtractorKind::Query,
                "Json" => ExtractorKind::Json,
                "Body" => ExtractorKind::Body,
                "Bytes" => ExtractorKind::Bytes,
                "BodyStream" => ExtractorKind::BodyStream,
                "Validated" => ExtractorKind::Validated,
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
/// Parts extractors (Path, Query, Request) are emitted first using
/// `FromRequestParts::from_request_parts(&req.parts)` — they borrow parts without
/// consuming `req`. Body extractors (Json, Body, Bytes, BodyStream, Validated) are
/// emitted last using `FromRequest::from_request(req).await` — they consume `req`.
/// This ordering ensures the borrow of `req.parts` completes before `req` is moved.
pub fn generate_extractor_extractions(
    params: &[ExtractorParam],
) -> Result<(Vec<TokenStream>, Vec<TokenStream>)> {
    // Extractions must run parts-first so parts borrows end before the body is moved.
    // Call args must match the original method signature order — these orderings are independent.
    //
    // The execute template always emits `let (_req_parts, _req_body) = __req.0.into_parts();`
    // before these extractions. Parts extractors borrow `_req_parts`; body extractors
    // reconstruct `HttpRequest::from_parts(_req_parts, _req_body)` and consume them.
    let mut ordered: Vec<&ExtractorParam> = params.iter().collect();
    ordered.sort_by_key(|p| p.kind.is_body_consuming() as u8);

    // The macro detects extractors by literal type name, so an aliased import
    // (e.g. `use toni::extractors::Bytes as Foo`) lands in `Unknown`. Pass the
    // real body to a sole `Unknown` extractor when no named body-consuming
    // extractor is competing for it — otherwise stay on the safe path of giving
    // each `Unknown` an empty body so multiple custom parts-only extractors
    // can coexist.
    let known_body_count = params
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                ExtractorKind::Json
                    | ExtractorKind::Body
                    | ExtractorKind::Bytes
                    | ExtractorKind::BodyStream
                    | ExtractorKind::Validated
                    | ExtractorKind::HttpRequest
            )
        })
        .count();
    let unknown_count = params
        .iter()
        .filter(|p| matches!(p.kind, ExtractorKind::Unknown))
        .count();
    let unknown_gets_body = known_body_count == 0 && unknown_count == 1;

    let mut extractions = Vec::new();

    for param in &ordered {
        let param_name = &param.param_name;
        let param_type = &param.param_type;

        match &param.kind {
            ExtractorKind::HttpRequest => {}

            // Not extracted from the request — the dispatcher hands it to the wrapper.
            ExtractorKind::Context => {
                let extraction = quote! { let #param_name = __ctx; };
                extractions.push(extraction);
            }

            // Returns None on extraction failure instead of a 400 response.
            ExtractorKind::Optional {
                inner_type,
                inner_kind,
            } => {
                let extraction = match inner_kind.as_ref() {
                    ExtractorKind::Unknown => quote! {
                        let #param_name = <#inner_type as ::toni::FromRequest>::from_request(
                            ::toni::http_helpers::HttpRequest::from_parts(
                                _req_parts.clone(),
                                ::toni::http_helpers::RequestBody::empty(),
                            )
                        ).await.ok();
                    },
                    k if k.is_body_consuming() => quote! {
                        let #param_name = <#inner_type as ::toni::FromRequest>::from_request(
                            ::toni::http_helpers::HttpRequest::from_parts(_req_parts, _req_body)
                        ).await.ok();
                    },
                    _ => quote! {
                        let #param_name = <#inner_type as ::toni::FromRequestParts>::from_request_parts(&_req_parts).ok();
                    },
                };
                extractions.push(extraction);
            }

            ExtractorKind::Path
            | ExtractorKind::Query
            | ExtractorKind::Request
            | ExtractorKind::Extensions => {
                let extraction = quote! {
                    let #param_name = match <#param_type as ::toni::FromRequestParts>::from_request_parts(&_req_parts) {
                        Ok(value) => value,
                        Err(e) => {
                            let error_body = ::toni::serde_json::json!({
                                "error": "Extraction failed",
                                "details": e.to_string()
                            });
                            return ::toni::http_helpers::ExecutionResult::Ok(::toni::http_helpers::HttpResponse {
                                body: Some(::toni::http_helpers::Body::json(error_body)),
                                status: 400,
                                headers: vec![],
                            });
                        }
                    };
                };
                extractions.push(extraction);
            }

            ExtractorKind::Json
            | ExtractorKind::Body
            | ExtractorKind::Bytes
            | ExtractorKind::BodyStream
            | ExtractorKind::Validated => {
                let extraction = quote! {
                    let #param_name = match <#param_type as ::toni::FromRequest>::from_request(
                        ::toni::http_helpers::HttpRequest::from_parts(_req_parts, _req_body)
                    ).await {
                        Ok(value) => value,
                        Err(e) => {
                            let error_body = ::toni::serde_json::json!({
                                "error": "Extraction failed",
                                "details": e.to_string()
                            });
                            return ::toni::http_helpers::ExecutionResult::Ok(::toni::http_helpers::HttpResponse {
                                body: Some(::toni::http_helpers::Body::json(error_body)),
                                status: 400,
                                headers: vec![],
                            });
                        }
                    };
                };
                extractions.push(extraction);
            }

            // Unknown types implement FromRequest. When this is the sole body-eligible
            // extractor in the handler, hand it the real streaming body so aliased imports
            // of body-consuming extractors (e.g. `use Bytes as Foo`) work the same as the
            // un-aliased name. Otherwise pass `RequestBody::empty()` so multiple custom
            // parts-only extractors can coexist.
            ExtractorKind::Unknown => {
                let extraction = if unknown_gets_body {
                    quote! {
                        let #param_name = match <#param_type as ::toni::FromRequest>::from_request(
                            ::toni::http_helpers::HttpRequest::from_parts(_req_parts, _req_body)
                        ).await {
                            Ok(value) => value,
                            Err(e) => {
                                let error_body = ::toni::serde_json::json!({
                                    "error": "Extraction failed",
                                    "details": e.to_string()
                                });
                                return ::toni::http_helpers::ExecutionResult::Ok(::toni::http_helpers::HttpResponse {
                                    body: Some(::toni::http_helpers::Body::json(error_body)),
                                    status: 400,
                                    headers: vec![],
                                });
                            }
                        };
                    }
                } else {
                    quote! {
                        let #param_name = match <#param_type as ::toni::FromRequest>::from_request(
                            ::toni::http_helpers::HttpRequest::from_parts(
                                _req_parts.clone(),
                                ::toni::http_helpers::RequestBody::empty(),
                            )
                        ).await {
                            Ok(value) => value,
                            Err(e) => {
                                let error_body = ::toni::serde_json::json!({
                                    "error": "Extraction failed",
                                    "details": e.to_string()
                                });
                                return ::toni::http_helpers::ExecutionResult::Ok(::toni::http_helpers::HttpResponse {
                                    body: Some(::toni::http_helpers::Body::json(error_body)),
                                    status: 400,
                                    headers: vec![],
                                });
                            }
                        };
                    }
                };
                extractions.push(extraction);
            }
        }
    }

    // Call args follow the original signature order, not the extraction order.
    let call_args: Vec<TokenStream> = params
        .iter()
        .map(|p| {
            let name = &p.param_name;
            match &p.kind {
                ExtractorKind::HttpRequest => quote! {
                    ::toni::http_helpers::HttpRequest::from_parts(_req_parts, _req_body)
                },
                _ => quote! { #name },
            }
        })
        .collect();

    Ok((extractions, call_args))
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
