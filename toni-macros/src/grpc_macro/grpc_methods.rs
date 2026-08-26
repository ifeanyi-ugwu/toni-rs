//! `#[grpc_methods]` — applied to `impl SomeProtoTrait for MyService` blocks.
//!
//! Emits three things alongside the user's impl block:
//!
//! 1. A `MyServiceGrpcServiceSource` companion carrying the service's declarations — its token and
//!    its enhancer tokens — and an `instance` that answers with the service serving a given call.
//!    Registration and enhancer resolution both happen before any call exists, which is why they
//!    read the companion rather than a service.
//!
//! 2. An `impl GrpcServiceSource` on that companion whose `register_with` body constructs a hidden
//!    enhancer-aware wrapper struct, downcasts the registrar to `tonic::service::RoutesBuilder`,
//!    and adds `MyServiceServer::new(wrapper)`.
//!
//! 3. A second `impl SomeProtoTrait for __MyServiceEnhanced` on the wrapper that runs guards
//!    (and, in later PRs, interceptors / error handlers) before delegating to the user's
//!    implementation via UFCS: `<MyService as SomeProtoTrait>::method(&inner, req).await`. UFCS
//!    keeps the user's body verbatim so `Self::SomeStream` associated types, `self.<field>`, and
//!    any inherent helper calls all resolve naturally.
//!
//! By convention the wrapping `*Server` type name is the proto trait's
//! identifier with `Server` appended (`OrdersService` → `OrdersServer`),
//! resolved in the trait's parent path. Override with
//! `#[grpc_methods(server = path::to::OrdersServer)]` when the
//! tonic-generated wrapper lives elsewhere or has a non-standard name.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemImpl, Path, Result, Token, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute,
};
use crate::shared::attr_is;
use crate::shared::set_metadata::{merged_metadata_exprs, metadata_ctor};

/// The `GrpcServiceSource` companion generated beside the service struct.
pub fn grpc_source_ident(self_ident: &syn::Ident) -> syn::Ident {
    format_ident!("{}GrpcServiceSource", self_ident)
}

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

    let server_path = args
        .server
        .unwrap_or_else(|| infer_server_path(&trait_path));

    let token = self_ident.to_string();
    let trait_short = trait_path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();

    let wrapper_ident = format_ident!("__{}Enhanced", self_ident);
    let source_ident = grpc_source_ident(&self_ident);

    // ── parse enhancer attrs (block-level + per-method) ─────────────────────
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_enhancer_infos = create_enhancer_infos(ctrl_enhancers_attr, Vec::new())?;
    let empty_vec = Vec::new();
    let ctrl_guard_tokens: Vec<_> = ctrl_enhancer_infos
        .get("guards")
        .unwrap_or(&empty_vec)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();
    let ctrl_interceptor_tokens: Vec<_> = ctrl_enhancer_infos
        .get("interceptors")
        .unwrap_or(&empty_vec)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();
    let ctrl_error_handler_tokens: Vec<_> = ctrl_enhancer_infos
        .get("error_handlers")
        .unwrap_or(&empty_vec)
        .iter()
        .filter(|i| !i.token_expr.is_empty())
        .map(|i| &i.token_expr)
        .collect();

    // One entry per method that carries any per-method enhancer attribute.
    // `get_handler_methods` returns the names so the resolver knows which
    // methods to query the per-handler getters for.
    let mut handler_enhancer_entries: Vec<(
        String,
        Vec<TokenStream>,
        Vec<TokenStream>,
        Vec<TokenStream>,
    )> = Vec::new();
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
                    let infos = create_enhancer_infos(method_attr, Vec::new())?;
                    let guards: Vec<TokenStream> = infos
                        .get("guards")
                        .unwrap_or(&empty_vec)
                        .iter()
                        .filter(|i| !i.token_expr.is_empty())
                        .map(|i| i.token_expr.clone())
                        .collect();
                    let interceptors: Vec<TokenStream> = infos
                        .get("interceptors")
                        .unwrap_or(&empty_vec)
                        .iter()
                        .filter(|i| !i.token_expr.is_empty())
                        .map(|i| i.token_expr.clone())
                        .collect();
                    let error_handlers: Vec<TokenStream> = infos
                        .get("error_handlers")
                        .unwrap_or(&empty_vec)
                        .iter()
                        .filter(|i| !i.token_expr.is_empty())
                        .map(|i| i.token_expr.clone())
                        .collect();
                    if !guards.is_empty() || !interceptors.is_empty() || !error_handlers.is_empty()
                    {
                        handler_enhancer_entries.push((
                            method_name,
                            guards,
                            interceptors,
                            error_handlers,
                        ));
                    }
                }
            }
            syn::ImplItem::Type(at) => assoc_types.push(at),
            other => other_items.push(other),
        }
    }

    // ── strip enhancer attrs from the user's impl block before re-emitting ──
    let mut user_impl = impl_block.clone();
    user_impl
        .attrs
        .retain(|attr| !has_enhancer_attribute(attr) && !attr_is(attr, "set_metadata"));
    for item in user_impl.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            method
                .attrs
                .retain(|attr| !has_enhancer_attribute(attr) && !attr_is(attr, "set_metadata"));
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

    let ctrl_interceptor_tokens_impl = if !ctrl_interceptor_tokens.is_empty() {
        quote! {
            fn get_interceptor_tokens(&self) -> ::std::vec::Vec<::std::string::String> {
                vec![#(#ctrl_interceptor_tokens),*]
            }
        }
    } else {
        quote! {}
    };

    let ctrl_error_handler_tokens_impl = if !ctrl_error_handler_tokens.is_empty() {
        quote! {
            fn get_error_handler_tokens(&self) -> ::std::vec::Vec<::std::string::String> {
                vec![#(#ctrl_error_handler_tokens),*]
            }
        }
    } else {
        quote! {}
    };

    let handler_methods_impl = if !handler_enhancer_entries.is_empty() {
        let names: Vec<&str> = handler_enhancer_entries
            .iter()
            .map(|(n, _, _, _)| n.as_str())
            .collect();
        quote! {
            fn get_handler_methods(&self) -> ::std::vec::Vec<::std::string::String> {
                vec![#(#names.to_string()),*]
            }
        }
    } else {
        quote! {}
    };

    let handler_guard_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, g, _, _)| !g.is_empty())
            .map(|(name, guards, _, _)| quote! { #name => vec![#(#guards),*], })
            .collect();
        if !arms.is_empty() {
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
        }
    };

    let handler_interceptor_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, _, i, _)| !i.is_empty())
            .map(|(name, _, interceptors, _)| quote! { #name => vec![#(#interceptors),*], })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_interceptor_tokens(&self, method: &str) -> ::std::vec::Vec<::std::string::String> {
                    match method {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
    };

    let handler_error_handler_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, _, _, e)| !e.is_empty())
            .map(|(name, _, _, handlers)| quote! { #name => vec![#(#handlers),*], })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_error_handler_tokens(&self, method: &str) -> ::std::vec::Vec<::std::string::String> {
                    match method {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
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
        .map(|method| {
            build_wrapper_method(
                method,
                &self_ident,
                &trait_path,
                &trait_short,
                &impl_block.attrs,
            )
        })
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

    let wrapper_other_items: Vec<TokenStream> =
        other_items.iter().map(|item| quote! { #item }).collect();

    let wrapper_def = quote! {
        #[doc(hidden)]
        #[derive(::std::clone::Clone)]
        pub struct #wrapper_ident {
            source: #source_ident,
            enhancers: ::std::sync::Arc<::toni::adapter::ResolvedGrpcEnhancers>,
        }

        #(#trait_attrs)*
        impl #trait_path for #wrapper_ident {
            #(#wrapper_assoc_types)*
            #(#wrapper_other_items)*
            #(#wrapper_methods)*
        }
    };

    // ── The source companion, and `GrpcServiceSource` on it ────────────────
    let grpc_trait_impl = quote! {
        #[doc(hidden)]
        #[derive(::std::clone::Clone)]
        pub enum #source_ident {
            /// Built at startup and shared by every call.
            Singleton(::std::sync::Arc<#self_ident>),
            /// The service's own provider, resolved inside the call being served.
            PerCall(::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>),
        }

        impl #source_ident {
            /// The service serving `ctx`.
            ///
            /// Inherent rather than a trait method: the wrapper delegates through UFCS at the
            /// concrete type, so what comes back has to be the service itself.
            #[doc(hidden)]
            pub async fn instance(
                &self,
                ctx: &::toni::context::GrpcContext,
            ) -> ::std::sync::Arc<#self_ident> {
                match self {
                    Self::Singleton(__instance) => __instance.clone(),
                    Self::PerCall(__provider) => {
                        // The provider caches in the execution, so a service asked for twice in one
                        // call is built once; init/bootstrap fire on the instance the call is served
                        // by, as they do for a request-scoped HTTP controller.
                        let __any = __provider
                            .execute(vec![], ::toni::ProviderContext::Grpc(ctx.clone()))
                            .await;
                        let __concrete = *__any.downcast::<#self_ident>().unwrap_or_else(|_| panic!(
                            "gRPC service '{}' resolved to a different type",
                            #token
                        ));
                        {
                            use ::toni::__lifecycle::LifecycleBridge as _;
                            let _ = #self_ident::__toni_lc_on_init(&__concrete).await;
                            let _ = #self_ident::__toni_lc_on_bootstrap(&__concrete).await;
                        }
                        ::std::sync::Arc::new(__concrete)
                    }
                }
            }
        }

        impl ::toni::adapter::GrpcServiceSource for #source_ident {
            fn token(&self) -> ::std::string::String {
                #token.to_string()
            }

            #ctrl_guard_tokens_impl
            #ctrl_interceptor_tokens_impl
            #ctrl_error_handler_tokens_impl
            #handler_methods_impl
            #handler_guard_tokens_impl
            #handler_interceptor_tokens_impl
            #handler_error_handler_tokens_impl

            fn register_with(
                &self,
                registrar: &mut dyn ::std::any::Any,
                enhancers: ::std::sync::Arc<::toni::adapter::ResolvedGrpcEnhancers>,
            ) {
                if let ::std::option::Option::Some(builder) = registrar.downcast_mut::<
                    ::tonic::service::RoutesBuilder,
                >() {
                    let __wrapper = #wrapper_ident {
                        source: self.clone(),
                        enhancers,
                    };
                    builder.add_service(#server_path::new(__wrapper));
                } else {
                    ::toni::tracing::warn!(
                        service = #token,
                        proto_trait = #trait_short,
                        "GrpcServiceSource::register_with received an unknown registrar; service not bound"
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

/// Build the wrapper's proto-trait method body. Runs the full pipeline
/// (guards → interceptors → user delegation) via `run_grpc_pipeline`,
/// maps any short-circuit [`GrpcStatus`] to `tonic::Status`, and reads
/// the user's typed reply back from a side-channel set inside the
/// delegate closure (the chain runner can't be generic over the
/// per-method response type).
///
/// Delegation uses UFCS — `<UserType as ProtoTrait>::method(&inner, ...)`
/// — so the user's body's `self.<field>` accesses, `Self::SomeStream`
/// associated-type references, and any inherent-helper calls resolve in
/// the user's original impl context, unchanged by this rewrite.
fn build_wrapper_method(
    method: &syn::ImplItemFn,
    self_ident: &syn::Ident,
    trait_path: &Path,
    trait_short: &str,
    impl_attrs: &[syn::Attribute],
) -> Result<TokenStream> {
    let sig = &method.sig;
    let method_name_lit = sig.ident.to_string();
    let method_path_lit = format!("{}/{}", trait_short, method_name_lit);

    // The impl block's `#[set_metadata]` entries then the method's, merged here rather than at every
    // call. The map is built once and shared, the service having one shape for the process.
    let merged = merged_metadata_exprs(impl_attrs, &method.attrs)?;
    let declared_metadata = match metadata_ctor(&merged) {
        Some(ctor) => quote! {
            static __DECLARED: ::std::sync::OnceLock<
                ::std::sync::Arc<::toni::context::Metadata>
            > = ::std::sync::OnceLock::new();
            let __declared = ::std::option::Option::Some(
                __DECLARED.get_or_init(|| ::std::sync::Arc::new(#ctor)).clone(),
            );
        },
        None => quote! {
            let __declared: ::std::option::Option<
                ::std::sync::Arc<::toni::context::Metadata>
            > = ::std::option::Option::None;
        },
    };

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

    // The first non-receiver argument is the tonic Request — its metadata
    // and remote_addr come off a borrow, so we read both without
    // consuming the request before handing it to the user delegate.
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

    // The user's return type — used as the side-channel payload so the
    // typed `Result<Response<_>, Status>` survives the trip through the
    // chain runner (which can only thread `()` because the response shape
    // varies per method).
    let ret_ty = match output {
        syn::ReturnType::Type(_, ty) => quote! { #ty },
        syn::ReturnType::Default => quote! { () },
    };

    Ok(quote! {
        #asyncness fn #method_ident #generics (#inputs) #output {
            let __metadata = #req_ident.metadata().iter().filter_map(|kv| match kv {
                ::tonic::metadata::KeyAndValueRef::Ascii(k, v) => v
                    .to_str()
                    .ok()
                    .map(|s| (k.as_str().to_string(), s.to_string())),
                ::tonic::metadata::KeyAndValueRef::Binary(_, _) => None,
            }).collect::<::std::collections::HashMap<::std::string::String, ::std::string::String>>();
            #declared_metadata
            let __ctx = ::toni::context::GrpcContext::new(
                #method_path_lit,
                __metadata,
                #req_ident.remote_addr(),
                __declared,
            );

            // The handler receives the tonic request, never the context, so the
            // context's extension bag rides the request to reach it. A handle,
            // not a copy — the guards below write into the same bag.
            let mut #req_ident = #req_ident;
            #req_ident.extensions_mut().insert(
                ::toni::context::HandlerContext::extensions(&__ctx).clone()
            );

            // Two slots so the macro can distinguish a returned reply
            // (Ok or Err) from a caught panic, and feed the panic event
            // (not its synthesized status) to observers + the error chain.
            let __outcome: ::std::sync::Arc<::std::sync::Mutex<::std::option::Option<#ret_ty>>>
                = ::std::sync::Arc::new(::std::sync::Mutex::new(::std::option::Option::None));
            let __panic: ::std::sync::Arc<::std::sync::Mutex<::std::option::Option<::toni::PanicRecovered>>>
                = ::std::sync::Arc::new(::std::sync::Mutex::new(::std::option::Option::None));
            let __outcome_capture = __outcome.clone();
            let __panic_capture = __panic.clone();
            let __source = self.source.clone();
            let __build_ctx = __ctx.clone();

            let __pipeline = ::toni::grpc_runtime::run_grpc_pipeline(
                &__ctx,
                &self.enhancers,
                #method_name_lit,
                move || async move {
                    // The service is asked for here and nowhere earlier: a guard that rejects never
                    // builds one. Construction sits inside the same panic recovery as the handler
                    // body, so a panicking constructor renders a status rather than tearing down
                    // the connection.
                    let __caught = ::toni::grpc_runtime::catch_handler_panic(async move {
                        let __inner = __source.instance(&__build_ctx).await;
                        <#self_ident as #trait_path>::#method_ident(
                            &__inner, #(#forward_args),*
                        ).await
                    }).await;
                    match __caught {
                        ::std::result::Result::Ok(__reply) => {
                            *__outcome_capture.lock().expect("grpc pipeline outcome mutex poisoned") =
                                ::std::option::Option::Some(__reply);
                        }
                        ::std::result::Result::Err(__panic_event) => {
                            *__panic_capture.lock().expect("grpc pipeline panic mutex poisoned") =
                                ::std::option::Option::Some(__panic_event);
                        }
                    }
                },
            ).await;

            if let ::std::result::Result::Err(__status) = __pipeline {
                let __code = ::tonic::Code::from_i32(__status.code as i32);
                return ::std::result::Result::Err(::tonic::Status::new(__code, __status.message));
            }

            // Caught panic: route the typed `PanicRecovered` through the
            // error chain so observers see it. Chain falls back to
            // `Internal` carrying the panic message. The take is bound to
            // a local so the `MutexGuard` is dropped before the `.await`
            // — holding it across would make the wrapper future `!Send`.
            let __taken_panic = __panic
                .lock()
                .expect("grpc pipeline panic mutex poisoned")
                .take();
            if let ::std::option::Option::Some(__panic_event) = __taken_panic {
                let __mapped = ::toni::grpc_runtime::run_grpc_error_chain(
                    &__ctx, &self.enhancers, #method_name_lit, &__panic_event,
                ).await;
                return ::std::result::Result::Err(match __mapped {
                    ::std::option::Option::Some(__grpc) => {
                        let __code = ::tonic::Code::from_i32(__grpc.code as i32);
                        ::tonic::Status::new(__code, __grpc.message)
                    }
                    ::std::option::Option::None => ::tonic::Status::internal(format!(
                        "handler panicked: {}", __panic_event
                    )),
                });
            }

            let __taken_outcome = __outcome
                .lock()
                .expect("grpc pipeline outcome mutex poisoned")
                .take();
            match __taken_outcome {
                ::std::option::Option::Some(::std::result::Result::Ok(__reply)) => {
                    ::std::result::Result::Ok(__reply)
                }
                ::std::option::Option::Some(::std::result::Result::Err(__status)) => {
                    // User-returned `Err(Status)` is offered to the error
                    // chain. If a handler claims it, the claimed
                    // `GrpcStatus` becomes the wire reply; otherwise the
                    // original status passes through unchanged.
                    let __wrapped = ::toni::GrpcStatus {
                        code: ::toni::GrpcCode::from_i32(__status.code() as i32),
                        message: __status.message().to_string(),
                    };
                    let __mapped = ::toni::grpc_runtime::run_grpc_error_chain(
                        &__ctx, &self.enhancers, #method_name_lit, &__wrapped,
                    ).await;
                    ::std::result::Result::Err(match __mapped {
                        ::std::option::Option::Some(__grpc) => {
                            let __code = ::tonic::Code::from_i32(__grpc.code as i32);
                            ::tonic::Status::new(__code, __grpc.message)
                        }
                        ::std::option::Option::None => __status,
                    })
                }
                ::std::option::Option::None => ::std::result::Result::Err(::tonic::Status::internal(
                    "interceptor short-circuited the call without producing a response"
                )),
            }
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
