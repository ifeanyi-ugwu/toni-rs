//! `#[grpc_methods]` — applied to `impl SomeProtoTrait for MyService` blocks.
//!
//! Emits two things alongside the user's impl block:
//!
//! 1. An `impl GrpcServiceTrait for MyService` whose `register_with` body
//!    constructs a hidden enhancer-aware wrapper struct, downcasts the
//!    registrar to `tonic::service::RoutesBuilder`, and adds
//!    `MyServiceServer::new(wrapper)`.
//!
//! 2. A second `impl SomeProtoTrait for __MyServiceEnhanced` on the wrapper
//!    that runs guards (and, in later PRs, interceptors / pipes / error
//!    handlers) before delegating to the user's implementation via UFCS:
//!    `<MyService as SomeProtoTrait>::method(&self.inner, req).await`. UFCS
//!    keeps the user's body verbatim so `Self::SomeStream` associated types,
//!    `self.<field>`, and any inherent helper calls all resolve naturally.
//!
//! By convention the wrapping `*Server` type name is the proto trait's
//! identifier with `Server` appended (`OrdersService` → `OrdersServer`),
//! resolved in the trait's parent path. Override with
//! `#[grpc_methods(server = path::to::OrdersServer)]` when the
//! tonic-generated wrapper lives elsewhere or has a non-standard name.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemImpl, Path, Result, Token, parse2};

use crate::enhancer::enhancer::{create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute};

struct GrpcMethodsArgs {
    server: Option<Path>,
}

impl syn::parse::Parse for GrpcMethodsArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        if input.is_empty() {
            return Ok(GrpcMethodsArgs { server: None });
        }
        let key: syn::Ident = input.parse()?;
        if key != "server" {
            return Err(syn::Error::new(key.span(), "expected `server = path`"));
        }
        let _: Token![=] = input.parse()?;
        let server: Path = input.parse()?;
        Ok(GrpcMethodsArgs {
            server: Some(server),
        })
    }
}

pub fn handle_grpc_methods(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<GrpcMethodsArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let trait_path = impl_block
        .trait_
        .as_ref()
        .map(|(_, path, _)| path.clone())
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &impl_block,
                "#[grpc_methods] must annotate `impl SomeProtoTrait for YourService` \
                 — it does nothing on inherent impls",
            )
        })?;

    let self_ty = impl_block.self_ty.as_ref();
    let self_ident = match self_ty {
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new_spanned(self_ty, "self type has no segments"))?
            .ident
            .clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                self_ty,
                "#[grpc_methods] expects a named `Self` type (got a non-path type)",
            ));
        }
    };

    let server_path = args.server.unwrap_or_else(|| infer_server_path(&trait_path));

    let token = self_ident.to_string();
    let trait_short = trait_path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();

    let wrapper_ident = format_ident!("__{}Enhanced", self_ident);

    // ── parse enhancer attrs (block-level + per-method) ─────────────────────
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_enhancer_infos =
        create_enhancer_infos(ctrl_enhancers_attr, std::collections::HashMap::new())?;
    let empty_vec = Vec::new();
    let ctrl_guard_tokens: Vec<_> = ctrl_enhancer_infos
        .get("guards")
        .unwrap_or(&empty_vec)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();

    // Per-method guard tokens, keyed by the method's identifier (lowercase
    // matches what `get_handler_methods` returns and what the chain lookup
    // uses at runtime).
    let mut handler_guard_entries: Vec<(String, Vec<TokenStream>)> = Vec::new();
    let mut method_idents: Vec<&syn::Ident> = Vec::new();
    let mut method_sigs_for_wrapper: Vec<&syn::ImplItemFn> = Vec::new();
    let mut assoc_types: Vec<&syn::ImplItemType> = Vec::new();
    let mut other_items: Vec<&syn::ImplItem> = Vec::new();

    for item in &impl_block.items {
        match item {
            syn::ImplItem::Fn(method) => {
                method_idents.push(&method.sig.ident);
                method_sigs_for_wrapper.push(method);
                let method_name = method.sig.ident.to_string();
                let method_attr = get_enhancers_attr(&method.attrs)?;
                if !method_attr.is_empty() {
                    let infos =
                        create_enhancer_infos(method_attr, std::collections::HashMap::new())?;
                    let guards: Vec<TokenStream> = infos
                        .get("guards")
                        .unwrap_or(&empty_vec)
                        .iter()
                        .filter(|i| !i.token_expr.is_empty())
                        .map(|i| i.token_expr.clone())
                        .collect();
                    if !guards.is_empty() {
                        handler_guard_entries.push((method_name, guards));
                    }
                }
            }
            syn::ImplItem::Type(at) => assoc_types.push(at),
            other => other_items.push(other),
        }
    }

    // ── strip enhancer attrs from the user's impl block before re-emitting ──
    let mut user_impl = impl_block.clone();
    user_impl.attrs.retain(|attr| !has_enhancer_attribute(attr));
    for item in user_impl.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| !has_enhancer_attribute(attr));
        }
    }

    // ── token-getter impls (only emitted when non-empty so manual-impl users
    //    don't see surprising overrides) ──────────────────────────────────
    let ctrl_guard_tokens_impl = if !ctrl_guard_tokens.is_empty() {
        quote! {
            fn get_guard_tokens(&self) -> ::std::vec::Vec<::std::string::String> {
                vec![#(#ctrl_guard_tokens),*]
            }
        }
    } else {
        quote! {}
    };

    let handler_methods_impl = if !handler_guard_entries.is_empty() {
        let names: Vec<&str> = handler_guard_entries.iter().map(|(n, _)| n.as_str()).collect();
        quote! {
            fn get_handler_methods(&self) -> ::std::vec::Vec<::std::string::String> {
                vec![#(#names.to_string()),*]
            }
        }
    } else {
        quote! {}
    };

    let handler_guard_tokens_impl = if !handler_guard_entries.is_empty() {
        let arms = handler_guard_entries.iter().map(|(name, tokens)| {
            quote! { #name => vec![#(#tokens),*], }
        });
        quote! {
            fn get_handler_guard_tokens(&self, method: &str) -> ::std::vec::Vec<::std::string::String> {
                match method {
                    #(#arms)*
                    _ => vec![],
                }
            }
        }
    } else {
        quote! {}
    };

    // ── wrapper struct + Clone + proto-trait impl that delegates ────────────
    let trait_attrs: Vec<&syn::Attribute> = impl_block
        .attrs
        .iter()
        .filter(|attr| {
            // Carry attributes like `#[tonic::async_trait]` to the wrapper's
            // proto-trait impl; drop the enhancer markers (already consumed)
            // and `#[grpc_methods]` itself (we are it).
            let path = attr.path();
            !has_enhancer_attribute(attr) && !path.is_ident("grpc_methods")
        })
        .collect();

    let wrapper_methods: Vec<TokenStream> = method_sigs_for_wrapper
        .iter()
        .map(|method| build_wrapper_method(method, &self_ident, &trait_path, &trait_short))
        .collect::<Result<Vec<_>>>()?;

    let wrapper_assoc_types: Vec<TokenStream> = assoc_types
        .iter()
        .map(|at| {
            let ident = &at.ident;
            let generics = &at.generics;
            quote! {
                type #ident #generics = <#self_ident as #trait_path>::#ident;
            }
        })
        .collect();

    let wrapper_other_items: Vec<TokenStream> = other_items
        .iter()
        .map(|item| quote! { #item })
        .collect();

    let wrapper_def = quote! {
        #[doc(hidden)]
        #[derive(::std::clone::Clone)]
        pub struct #wrapper_ident {
            inner: ::std::sync::Arc<#self_ident>,
            enhancers: ::std::sync::Arc<::toni::adapter::ResolvedGrpcEnhancers>,
        }

        #(#trait_attrs)*
        impl #trait_path for #wrapper_ident {
            #(#wrapper_assoc_types)*
            #(#wrapper_other_items)*
            #(#wrapper_methods)*
        }
    };

    // ── GrpcServiceTrait impl on the user's struct ──────────────────────────
    let grpc_trait_impl = quote! {
        impl ::toni::adapter::GrpcServiceTrait for #self_ty {
            fn token(&self) -> ::std::string::String {
                #token.to_string()
            }

            #ctrl_guard_tokens_impl
            #handler_methods_impl
            #handler_guard_tokens_impl

            fn register_with(
                &self,
                registrar: &mut dyn ::std::any::Any,
                enhancers: ::std::sync::Arc<::toni::adapter::ResolvedGrpcEnhancers>,
            ) {
                if let ::std::option::Option::Some(builder) = registrar.downcast_mut::<
                    ::tonic::service::RoutesBuilder,
                >() {
                    let __wrapper = #wrapper_ident {
                        inner: ::std::sync::Arc::new(self.clone()),
                        enhancers,
                    };
                    builder.add_service(#server_path::new(__wrapper));
                } else {
                    ::toni::tracing::warn!(
                        service = #token,
                        proto_trait = #trait_short,
                        "GrpcServiceTrait::register_with received an unknown registrar; service not bound"
                    );
                }
            }
        }
    };

    Ok(quote! {
        #user_impl
        #wrapper_def
        #grpc_trait_impl
    })
}

/// Build the wrapper's proto-trait method body. Runs guards through
/// `run_grpc_guards`, maps any [`GrpcStatus`] to `tonic::Status`, then
/// delegates to the user's implementation via UFCS so the body's
/// `self.<field>` and associated-type references resolve unchanged.
fn build_wrapper_method(
    method: &syn::ImplItemFn,
    self_ident: &syn::Ident,
    trait_path: &Path,
    trait_short: &str,
) -> Result<TokenStream> {
    let sig = &method.sig;
    let method_name_lit = sig.ident.to_string();
    let method_path_lit = format!("{}/{}", trait_short, method_name_lit);

    // Forward every non-receiver argument to the user impl by name.
    let forward_args: Vec<TokenStream> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Receiver(_) => None,
            syn::FnArg::Typed(pt) => match pt.pat.as_ref() {
                syn::Pat::Ident(pi) => {
                    let ident = &pi.ident;
                    Some(quote! { #ident })
                }
                _ => Some(quote! { compile_error!("#[grpc_methods] requires named arguments") }),
            },
        })
        .collect();

    // The first non-receiver argument is the tonic Request — we only need
    // its metadata + remote_addr, both of which take `&Request<_>` so we
    // don't consume it before delegation.
    let req_ident = match sig.inputs.iter().nth(1) {
        Some(syn::FnArg::Typed(pt)) => match pt.pat.as_ref() {
            syn::Pat::Ident(pi) => &pi.ident,
            _ => {
                return Err(syn::Error::new_spanned(
                    pt,
                    "#[grpc_methods] expects the request argument to be a named binding",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "#[grpc_methods] expects `&self` followed by `Request<_>` (or `Request<Streaming<_>>`)",
            ));
        }
    };

    let method_ident = &sig.ident;
    let asyncness = sig.asyncness.as_ref();
    let inputs = &sig.inputs;
    let output = &sig.output;
    let generics = &sig.generics;

    Ok(quote! {
        #asyncness fn #method_ident #generics (#inputs) #output {
            let __metadata = #req_ident.metadata().iter().filter_map(|kv| match kv {
                ::tonic::metadata::KeyAndValueRef::Ascii(k, v) => v
                    .to_str()
                    .ok()
                    .map(|s| (k.as_str().to_string(), s.to_string())),
                ::tonic::metadata::KeyAndValueRef::Binary(_, _) => None,
            }).collect::<::std::collections::HashMap<::std::string::String, ::std::string::String>>();
            let mut __ctx = ::toni::context::GrpcContext::new(
                #method_path_lit,
                __metadata,
                #req_ident.remote_addr(),
                ::std::option::Option::None,
            );
            if let ::std::result::Result::Err(__status) =
                ::toni::grpc_runtime::run_grpc_guards(&mut __ctx, &self.enhancers, #method_name_lit).await
            {
                let __code = ::tonic::Code::from_i32(__status.code as i32);
                return ::std::result::Result::Err(::tonic::Status::new(__code, __status.message));
            }
            <#self_ident as #trait_path>::#method_ident(&self.inner, #(#forward_args),*).await
        }
    })
}

/// Convention: `OrdersService` (proto trait) → `OrdersServer` in the same
/// parent path. tonic-build emits `OrdersServer` alongside the trait, so
/// `parent::OrdersService` gives us `parent::OrdersServer`.
fn infer_server_path(trait_path: &Path) -> Path {
    let mut path = trait_path.clone();
    if let Some(last) = path.segments.last_mut() {
        let ident = last.ident.to_string();
        let base = ident.strip_suffix("Service").unwrap_or(&ident);
        let new_ident = format!("{}Server", base);
        last.ident = syn::Ident::new(&new_ident, last.ident.span());
        last.arguments = syn::PathArguments::None;
    }
    path
}

