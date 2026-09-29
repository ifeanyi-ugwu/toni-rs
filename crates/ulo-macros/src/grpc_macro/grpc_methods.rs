//! `#[grpc_methods(proto::Trait)]` — applied to the inherent impl holding a gRPC service's
//! handlers.
//!
//! A handler is written in ulo's shapes — `Payload<T>` or `Inbound<T>` for the request, the reply
//! message for the answer, a domain error for the failure — and the macro writes tonic's. Each one
//! is marked `#[grpc_method]` or, for a streaming reply, `#[grpc_stream]`; anything unmarked stays
//! in the inherent impl, which is where the constructor and `#[on_*]` hooks live.
//!
//! The generated signature names each method's request type through a marker ulo-build wrote
//! beside the trait — `<greeter_ulo::Greet as ulo_grpc::MethodShape>::Arg` — rather than by
//! reading anything off the handler: a macro runs before name resolution, so a parameter's type
//! is an identifier it cannot resolve (ADR-0043). The marker also installs the request on the
//! execution, and every parameter of the handler is then a `FromContext<GrpcContext>`, in any
//! order, with `&GrpcContext` passing through as it does on the other three transports.
//!
//! Lowering runs first: each handler keeps its body under `__ulo_grpc_<name>` and gains two
//! things — a hidden `__ulo_grpc_run_<name>(&self, &GrpcContext)` that extracts its parameters
//! from the execution, calls it and renders its answer, and a proto trait method that installs
//! the request on the execution and calls the run fn. The hidden names are what keep the
//! generated method from resolving to itself. The enhancer wrapper below installs the request
//! itself, before the guards, and calls the run fn directly. From there the rest of this module
//! sees an ordinary trait impl, and emits three things alongside it:
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
//! 3. A second `impl SomeProtoTrait for __MyServiceEnhanced` on the wrapper that runs guards,
//!    interceptors and error handlers before delegating to the generated implementation via UFCS:
//!    `<MyService as SomeProtoTrait>::method(&inner, req).await`.
//!
//! # Streaming replies
//!
//! The wrapper declares its own associated stream types — `ScopedGrpcStream<GeneratedStream>`
//! rather than an alias of the generated one — so a reply that outlives the handler carries the
//! execution with it and fires its cancellation token if the caller abandons it. The generated impl
//! writes each streaming response as `Self::SomeStream`, so those response types are what say which
//! methods stream.
//!
//! tonic-build derives a method and its associated type from one proto identifier, so `#[grpc_stream]`
//! reads the name from the method: `watch_progress` declares `WatchProgressStream`. A trait whose
//! own naming does not connect the two — written by hand, or built through `tonic_build::manual`,
//! where the Rust name and the route name are set independently — names it on the attribute:
//! `#[grpc_stream(StreamProgressStream)]`.
//!
//! By convention the wrapping `*Server` type name is the proto trait's identifier with `Server`
//! appended (`OrdersService` → `OrdersServer`), resolved in the trait's parent path. Override with
//! `#[grpc_methods(orders_server::Orders, server = path::to::OrdersServer)]` when the
//! tonic-generated wrapper lives elsewhere or has a non-standard name.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemImpl, Path, Result, Token, parse2};

use crate::enhancer::enhancer::{
    create_enhancer_infos, declares_anything, enhancer_entries, get_enhancers_attr,
    has_enhancer_attribute,
};
use crate::shared::attr_is;
use crate::shared::set_metadata::{merged_metadata_exprs, metadata_ctor};

/// The `GrpcServiceSource` companion generated beside the service struct — a newtype over
/// `DispatchSource<Service>`, local to the expansion crate because a foreign trait cannot be
/// implemented on the foreign `DispatchSource` directly.
pub fn grpc_source_ident(self_ident: &syn::Ident) -> syn::Ident {
    format_ident!("{}GrpcServiceSource", self_ident)
}

struct GrpcMethodsArgs {
    /// The proto trait to implement, named when the macro writes the impl
    /// itself: `#[grpc_methods(orders_server::Orders)]`. A trait impl states
    /// it in its own header and leaves this empty.
    proto_trait: Option<Path>,
    server: Option<Path>,
    /// The module holding the method markers ulo-build wrote. Defaults to
    /// `{trait_snake}_ulo` beside the trait's module, which is where
    /// `ulo_build::shapes` puts it.
    shapes: Option<Path>,
}

impl syn::parse::Parse for GrpcMethodsArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut proto_trait: Option<Path> = None;
        let mut server: Option<Path> = None;
        let mut shapes: Option<Path> = None;

        while !input.is_empty() {
            let fork = input.fork();
            let keyed = fork
                .parse::<syn::Ident>()
                .ok()
                .filter(|key| (key == "server" || key == "shapes") && fork.peek(Token![=]));

            if let Some(key) = keyed {
                let _key: syn::Ident = input.parse()?;
                let _: Token![=] = input.parse()?;
                if key == "server" {
                    server = Some(input.parse()?);
                } else {
                    shapes = Some(input.parse()?);
                }
            } else {
                let path: Path = input.parse()?;
                if proto_trait.is_some() {
                    return Err(syn::Error::new_spanned(
                        path,
                        "#[grpc_methods] takes one proto trait; the second path is unexpected",
                    ));
                }
                proto_trait = Some(path);
            }

            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            } else {
                break;
            }
        }

        Ok(GrpcMethodsArgs {
            proto_trait,
            server,
            shapes,
        })
    }
}

pub fn handle_grpc_methods(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<GrpcMethodsArgs>(attr)?;
    let written = parse2::<ItemImpl>(item)?;

    // The handlers live in an inherent impl and speak ulo's shapes; the proto
    // trait impl around them is what this macro writes.
    if written.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &written,
            "#[grpc_methods] goes on the inherent impl that holds the handlers, naming the \
             proto trait it serves — `#[grpc_methods(orders_server::Orders)] impl MyService`. \
             Each handler is marked `#[grpc_method]` or `#[grpc_stream]`, takes what it needs \
             as extractors (`Payload<T>`, `Inbound<T>`, `Extensions`, `&GrpcContext`), and \
             answers with the reply message.",
        ));
    }

    let proto_trait = args.proto_trait.clone().ok_or_else(|| {
        syn::Error::new_spanned(
            &written,
            "#[grpc_methods] needs the proto trait it serves \
             — `#[grpc_methods(orders_server::Orders)]`",
        )
    })?;
    let shapes = args
        .shapes
        .clone()
        .unwrap_or_else(|| infer_shapes_path(&proto_trait));
    let (impl_block, handlers_impl) = lower_handlers_impl(&written, &proto_trait, &shapes)?;

    let trait_path = impl_block
        .trait_
        .as_ref()
        .map(|(_, path, _)| path.clone())
        .expect("the generated impl names the proto trait");

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
    let transport = quote! { ::ulo::dispatch::Grpc };
    let ctrl_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let ctrl_enhancer_infos = create_enhancer_infos(ctrl_enhancers_attr, Vec::new())?;
    let ctrl_guards = enhancer_entries(&ctrl_enhancer_infos, "guards", &transport);
    let ctrl_interceptors = enhancer_entries(&ctrl_enhancer_infos, "interceptors", &transport);
    let ctrl_error_handlers = enhancer_entries(&ctrl_enhancer_infos, "error_handlers", &transport);

    // One entry per method that carries any per-method enhancer attribute; each becomes a
    // `GrpcHandlerEnhancers` in the descriptor, keyed by the method's Rust name.
    let mut handler_entries: Vec<TokenStream> = Vec::new();
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
                    if declares_anything(&infos) {
                        let guards = enhancer_entries(&infos, "guards", &transport);
                        let interceptors = enhancer_entries(&infos, "interceptors", &transport);
                        let error_handlers = enhancer_entries(&infos, "error_handlers", &transport);
                        handler_entries.push(quote! {
                            ::ulo::grpc::GrpcHandlerEnhancers {
                                method: #method_name.to_string(),
                                guards: vec![#(#guards),*],
                                interceptors: vec![#(#interceptors),*],
                                error_handlers: vec![#(#error_handlers),*],
                            }
                        });
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
            method.attrs.retain(|attr| {
                !has_enhancer_attribute(attr)
                    && !attr_is(attr, "set_metadata")
                    && !attr_is(attr, "stream")
            });
        }
    }

    // ── one descriptor, emitted only when the service declares something ───
    let enhancers_impl = if !declares_anything(&ctrl_enhancer_infos) && handler_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn enhancers(&self) -> ::ulo::grpc::GrpcEnhancers {
                ::ulo::grpc::GrpcEnhancers {
                    guards: vec![#(#ctrl_guards),*],
                    interceptors: vec![#(#ctrl_interceptors),*],
                    error_handlers: vec![#(#ctrl_error_handlers),*],
                    handlers: vec![#(#handler_entries),*],
                }
            }
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

    // Which associated types carry a streaming reply. The generated impl names
    // each one as `Self::X` in the response type, so the response types are the
    // whole signal.
    let assoc_idents: std::collections::HashSet<String> =
        assoc_types.iter().map(|at| at.ident.to_string()).collect();
    let mut streaming_assocs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for method in &method_sigs_for_wrapper {
        if let syn::ReturnType::Type(_, ty) = &method.sig.output {
            collect_self_assoc(ty, &assoc_idents, &mut streaming_assocs);
        }
    }

    let wrapper_methods: Vec<TokenStream> = method_sigs_for_wrapper
        .iter()
        .map(|method| {
            build_wrapper_method(
                method,
                &self_ident,
                &trait_short,
                &impl_block.attrs,
                &shapes,
                &trait_path,
                &assoc_idents,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let wrapper_assoc_types: Vec<TokenStream> = assoc_types
        .iter()
        .map(|at| {
            let ident = &at.ident;
            let generics = &at.generics;
            if streaming_assocs.contains(&ident.to_string()) {
                quote! {
                    type #ident #generics = ::ulo::__grpc::ScopedGrpcStream<
                        <#self_ident as #trait_path>::#ident
                    >;
                }
            } else {
                quote! {
                    type #ident #generics = <#self_ident as #trait_path>::#ident;
                }
            }
        })
        .collect();

    let wrapper_other_items: Vec<TokenStream> =
        other_items.iter().map(|item| quote! { #item }).collect();

    let wrapper_def = quote! {
        #[doc(hidden)]
        #[derive(::std::clone::Clone)]
        pub struct #wrapper_ident {
            source: ::ulo::__enhancer::DispatchSource<#self_ident>,
            enhancers: ::std::sync::Arc<::ulo::grpc::ResolvedGrpcEnhancers>,
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
        pub struct #source_ident(::ulo::__enhancer::DispatchSource<#self_ident>);

        impl #self_ident {
            /// Shadows the `DispatchBridge` default: this controller dispatches gRPC.
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            pub fn __ulo_dispatch(
                source: &::ulo::__enhancer::DispatchSource<#self_ident>,
            ) -> ::ulo::dispatch::Targets {
                // The route prefix is HTTP's argument; a gRPC service cannot use one.
                if !<#self_ident>::__ulo_prefix().is_empty() {
                    ::ulo::tracing::warn!(
                        controller = #token,
                        prefix = <#self_ident>::__ulo_prefix(),
                        "controller dispatches gRPC; the route prefix is unused"
                    );
                }
                ::ulo::dispatch::Targets::Grpc(
                    ::std::sync::Arc::new(#source_ident(source.clone())),
                )
            }
        }

        impl ::ulo::grpc::GrpcServiceSource for #source_ident {
            fn token(&self) -> ::std::string::String {
                #token.to_string()
            }

            #enhancers_impl

            fn register_with(
                &self,
                registrar: &mut dyn ::std::any::Any,
                enhancers: ::std::sync::Arc<::ulo::grpc::ResolvedGrpcEnhancers>,
            ) {
                if let ::std::option::Option::Some(builder) = registrar.downcast_mut::<
                    ::tonic::service::RoutesBuilder,
                >() {
                    let __wrapper = #wrapper_ident {
                        source: self.0.clone(),
                        enhancers,
                    };
                    builder.add_service(#server_path::new(__wrapper));
                } else {
                    ::ulo::tracing::warn!(
                        service = #token,
                        proto_trait = #trait_short,
                        "GrpcServiceSource::register_with received an unknown registrar; service not bound"
                    );
                }
            }
        }
    };

    Ok(quote! {
        #handlers_impl
        #user_impl
        #wrapper_def
        #grpc_trait_impl
    })
}

/// Split an inherent impl into the handlers as the user wrote them and the
/// proto trait impl that calls them.
///
/// Everything a gRPC handler had to spell out — `Request` in, `Response` out,
/// a `Status` for an error — is written here instead, so a method reads the
/// way it does on the other three transports. The handlers keep their bodies
/// and move to a `__ulo_grpc_`-prefixed name, which is what the generated
/// trait method calls: same name in both impls would leave the call resolving
/// by inherent-first precedence, and a rename that ever slipped would recurse.
fn lower_handlers_impl(
    inherent: &ItemImpl,
    proto_trait: &Path,
    shapes: &Path,
) -> Result<(ItemImpl, ItemImpl)> {
    let mut handler_items: Vec<syn::ImplItem> = Vec::new();
    let mut generated_items: Vec<syn::ImplItem> = Vec::new();

    for item in &inherent.items {
        let syn::ImplItem::Fn(method) = item else {
            handler_items.push(item.clone());
            continue;
        };

        // `Some(None)` streams and takes the paired name; `Some(Some(ident))`
        // streams and names the associated type itself.
        let streams = match method
            .attrs
            .iter()
            .find(|attr| attr_is(attr, "grpc_stream"))
        {
            None => None,
            Some(attr) => Some(match attr.meta {
                syn::Meta::Path(_) => None,
                _ => Some(attr.parse_args::<syn::Ident>()?),
            }),
        };
        if streams.is_none() && !method.attrs.iter().any(|attr| attr_is(attr, "grpc_method")) {
            handler_items.push(item.clone());
            continue;
        }

        let (handler, run, generated) = lower_handler(method, streams, shapes, proto_trait)?;
        handler_items.push(syn::ImplItem::Fn(handler));
        handler_items.push(syn::ImplItem::Fn(run));
        generated_items.extend(generated);
    }

    if generated_items.is_empty() {
        return Err(syn::Error::new_spanned(
            inherent,
            "#[grpc_methods] found no `#[grpc_method]` or `#[grpc_stream]` handler in this impl",
        ));
    }

    let self_ty = inherent.self_ty.as_ref();

    let mut handlers = inherent.clone();
    handlers.items = handler_items;
    handlers.attrs.retain(|attr| {
        !has_enhancer_attribute(attr)
            && !attr_is(attr, "set_metadata")
            && !attr_is(attr, "grpc_methods")
    });

    let carried: Vec<&syn::Attribute> = inherent
        .attrs
        .iter()
        .filter(|attr| has_enhancer_attribute(attr) || attr_is(attr, "set_metadata"))
        .collect();

    let generated: ItemImpl = syn::parse_quote! {
        #(#carried)*
        #[::tonic::async_trait]
        impl #proto_trait for #self_ty {
            #(#generated_items)*
        }
    };

    Ok((generated, handlers))
}

/// `pkg::greeter_server::Greeter` → `pkg::greeter_ulo`, which is where
/// `ulo_build::shapes` writes the markers: beside the trait's module, named
/// from the trait the way tonic names its own modules.
fn infer_shapes_path(proto_trait: &Path) -> Path {
    let mut path = proto_trait.clone();
    let trait_ident = path
        .segments
        .pop()
        .map(|pair| pair.into_value().ident)
        .expect("a trait path has a last segment");
    // Drop the `*_server` module the trait lives in.
    path.segments.pop();
    path.segments.push(syn::PathSegment::from(format_ident!(
        "{}_ulo",
        to_snake(&trait_ident.to_string())
    )));
    path
}

/// tonic-build's own snake-casing, copied so the derived module name is the
/// one ulo-build wrote; the two crates test the same table.
fn to_snake(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c.to_ascii_lowercase());
        if chars.peek().is_some_and(|next| next.is_uppercase()) {
            out.push('_');
        }
    }
    out
}

/// `greet_all` names the marker `GreetAll`, as ulo-build wrote it.
fn to_upper_camel(ident: &str) -> String {
    ident
        .trim_start_matches("r#")
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The request is taken once — a stream has nothing to hand a second reader,
/// and a message follows the same rule. Which parameters take it comes off
/// `FromContext::CONSUMES`, one assertion per pair so the message names both;
/// `#[routes]` does the same for the HTTP body.
fn one_taker_assertion(params: &[(syn::Ident, syn::Type)]) -> TokenStream {
    let mut assertions = Vec::new();
    for (i, (first_name, first_ty)) in params.iter().enumerate() {
        for (second_name, second_ty) in params.iter().skip(i + 1) {
            let message = format!(
                "`{first_name}` and `{second_name}` both take the request, and it can only be \
                 taken once.\nKeep one of them: `Payload<T>` for the message, `Inbound<T>` for \
                 the caller's stream, or `GrpcRequest<T>` for the whole request.",
            );
            assertions.push(quote! {
                const _: () = {
                    assert!(
                        !(<#first_ty as ::ulo::extract::FromContext<
                            ::ulo::grpc::GrpcContext,
                        >>::CONSUMES
                            && <#second_ty as ::ulo::extract::FromContext<
                                ::ulo::grpc::GrpcContext,
                            >>::CONSUMES),
                        #message
                    );
                };
            });
        }
    }
    quote! { #(#assertions)* }
}

/// The hidden entry the enhancer wrapper calls once the request is installed
/// and the guards have run: extraction, the handler, the rendering.
fn run_ident(name: &syn::Ident) -> syn::Ident {
    format_ident!("__ulo_grpc_run_{}", name)
}

/// `&GrpcContext` — the context itself, not something extracted from it, which
/// is how the other three transports read it too.
fn is_grpc_context_ref(ty: &syn::Type) -> bool {
    let syn::Type::Reference(reference) = ty else {
        return false;
    };
    matches!(reference.elem.as_ref(), syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "GrpcContext"))
}

/// Rewrite one handler into itself under a hidden name, plus the proto-trait
/// items that extract its arguments and render its answer — the method, and
/// for a streaming reply the associated type the trait declares for it.
fn lower_handler(
    method: &syn::ImplItemFn,
    streams: Option<Option<syn::Ident>>,
    shapes: &Path,
    proto_trait: &Path,
) -> Result<(syn::ImplItemFn, syn::ImplItemFn, Vec<syn::ImplItem>)> {
    let name = &method.sig.ident;
    let hidden = format_ident!("__ulo_grpc_{}", name);
    let run = run_ident(name);

    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "a gRPC handler is async",
        ));
    }

    // What the wire carries is asked of the method's marker, not of the handler:
    // the trait's request type is a fact of the proto, and ulo-build wrote it
    // beside the trait where a projection can reach it (ADR-0043).
    let marker = format_ident!("{}", to_upper_camel(&name.to_string()));
    let shape = quote! { #shapes::#marker };

    // Every parameter is read from the context, the way a handler's parameters
    // are read on the other three transports. The request is one of them.
    let mut call_args: Vec<TokenStream> = Vec::new();
    let mut extractions: Vec<TokenStream> = Vec::new();
    let mut extracted: Vec<(syn::Ident, syn::Type)> = Vec::new();

    for arg in &method.sig.inputs {
        let syn::FnArg::Typed(typed) = arg else {
            continue;
        };
        let ty = typed.ty.as_ref();
        if is_grpc_context_ref(ty) {
            call_args.push(quote! { __ctx });
            continue;
        }
        let name = crate::controller_macro::extractor_params::extract_param_name(&typed.pat)
            .ok_or_else(|| {
                syn::Error::new_spanned(typed, "a gRPC handler's parameters are named")
            })?;
        extractions.push(quote! {
            let #name = match <#ty as ::ulo::extract::FromContext<
                ::ulo::grpc::GrpcContext,
            >>::extract(__ctx).await {
                ::std::result::Result::Ok(__value) => __value,
                ::std::result::Result::Err(__e) => {
                    return ::std::result::Result::Err(
                        ::ulo::grpc::GrpcStatus::internal(__e.to_string()),
                    );
                }
            };
        });
        call_args.push(quote! { #name });
        extracted.push((name, ty.clone()));
    }

    let one_taker = one_taker_assertion(&extracted);
    let request_arg_ty = quote! { <#shape as ::ulo_grpc::MethodShape>::Arg };

    // The trait method is the door tonic knocks on when no wrapper stands in
    // front of it: it installs the request and runs. Under the enhancer
    // wrapper the request is installed before the guards, and the wrapper
    // calls the run fn directly.
    let install_and_run = quote! {
        let __ctx = match ::ulo::grpc::GrpcContext::of(request.extensions()) {
            ::std::option::Option::Some(__ctx) => __ctx,
            ::std::option::Option::None => {
                return ::std::result::Result::Err(::tonic::Status::internal(
                    "this method was reached outside ulo's dispatch: no execution rides the request",
                ));
            }
        };
        <#shape as ::ulo_grpc::MethodShape>::install(request, &__ctx);
        // The one place a status flattens into tonic's: reached without the wrapper, there is no
        // chain above to be offered the error the status carries, so it rides out on the source
        // slot as `GrpcFailure` instead.
        ::std::result::Result::map_err(Self::#run(self, &__ctx).await, ::ulo_grpc::to_status)
    };
    let bind_params = quote! {
        #one_taker
        #(#extractions)*
    };

    // A handler that cannot fail answers with the reply itself, as an HTTP
    // handler returning a bare `Body` does.
    let (answer_ty, fallible) = answer_type(&method.sig.output)?;

    // A handler that answers `Response<T>` built the reply itself — metadata,
    // extensions and all — so the generated method passes it through rather
    // than wrapping the value a second time.
    let carried_response = response_inner_type(&answer_ty);
    let answer_ty = carried_response.clone().unwrap_or(answer_ty);

    let carried: Vec<&syn::Attribute> = method
        .attrs
        .iter()
        .filter(|attr| has_enhancer_attribute(attr) || attr_is(attr, "set_metadata"))
        .collect();

    // The error arm is the same whichever shape the reply takes. `of` keeps the error on the
    // status, which is what lets the chain above see its type rather than the code it mapped to.
    let failure = quote! {
        ::std::result::Result::Err(::ulo::grpc::GrpcStatus::of(__err))
    };

    let wrap_reply = if carried_response.is_some() {
        quote! { __reply }
    } else {
        quote! { ::tonic::Response::new(__reply) }
    };

    // The stream is boxed either way; a carried response keeps its parts.
    let wrap_stream = if carried_response.is_some() {
        quote! {{
            let (__meta, __body, __ext) = __reply.into_parts();
            let __mapped = ::ulo::futures::StreamExt::map(__body, __map_item);
            ::tonic::Response::from_parts(__meta, ::std::boxed::Box::pin(__mapped), __ext)
        }}
    } else {
        quote! {{
            let __mapped = ::ulo::futures::StreamExt::map(__reply, __map_item);
            ::tonic::Response::new(::std::boxed::Box::pin(__mapped))
        }}
    };

    let call_unary = if fallible {
        quote! {
            match Self::#hidden(self, #(#call_args),*).await {
                ::std::result::Result::Ok(__reply) => ::std::result::Result::Ok(#wrap_reply),
                ::std::result::Result::Err(__err) => { #failure }
            }
        }
    } else {
        quote! {
            let __reply = Self::#hidden(self, #(#call_args),*).await;
            ::std::result::Result::Ok(#wrap_reply)
        }
    };

    let call_stream = if fallible {
        quote! {
            match Self::#hidden(self, #(#call_args),*).await {
                ::std::result::Result::Ok(__reply) => ::std::result::Result::Ok(#wrap_stream),
                ::std::result::Result::Err(__err) => { #failure }
            }
        }
    } else {
        quote! {
            let __reply = Self::#hidden(self, #(#call_args),*).await;
            ::std::result::Result::Ok(#wrap_stream)
        }
    };

    let mut generated: Vec<syn::ImplItem> = Vec::new();

    let run_fn: syn::ImplItemFn;
    let generated_fn: syn::ImplItemFn = if let Some(named_assoc) = streams {
        // tonic names a streaming reply's associated type after the method, and
        // the trait declares it: `greet_many` pairs with `GreetManyStream`. A
        // trait built through `tonic_build::manual` sets the Rust name and the
        // route name independently, so there the method names it instead.
        let assoc = named_assoc.unwrap_or_else(|| assoc_stream_ident(name));
        let item_ty = stream_item_type(&answer_ty).ok_or_else(|| {
            syn::Error::new_spanned(
                &method.sig.output,
                "a `#[grpc_stream]` handler answers with \
                 `Result<impl Stream<Item = Result<Reply, YourError>> + Send + 'static, YourError>` \
                 — the item type is read from that `Item` binding",
            )
        })?;
        let reply_ty = result_ok_type(&item_ty).unwrap_or(item_ty);

        generated.push(syn::parse_quote! {
            type #assoc = ::std::pin::Pin<::std::boxed::Box<
                dyn ::ulo::futures::Stream<
                    Item = ::std::result::Result<#reply_ty, ::tonic::Status>,
                > + ::std::marker::Send,
            >>;
        });

        run_fn = syn::parse_quote! {
            #[doc(hidden)]
            async fn #run(
                &self,
                __ctx: &::ulo::grpc::GrpcContext,
            ) -> ::std::result::Result<
                ::tonic::Response<<Self as #proto_trait>::#assoc>,
                ::ulo::grpc::GrpcStatus,
            > {
                #bind_params
                // Each item carries the caller's own error type, which reaches
                // the wire as the code its kind means, with its detail. Only the
                // reply that opens the stream reaches the chain — an item failing
                // arrives after the answer has begun.
                let __map_item = |__item| {
                    ::std::result::Result::map_err(__item, ::ulo_grpc::to_status)
                };
                #call_stream
            }
        };

        syn::parse_quote! {
            #(#carried)*
            async fn #name(
                &self,
                request: ::tonic::Request<#request_arg_ty>,
            ) -> ::std::result::Result<::tonic::Response<Self::#assoc>, ::tonic::Status> {
                #install_and_run
            }
        }
    } else {
        run_fn = syn::parse_quote! {
            #[doc(hidden)]
            async fn #run(
                &self,
                __ctx: &::ulo::grpc::GrpcContext,
            ) -> ::std::result::Result<::tonic::Response<#answer_ty>, ::ulo::grpc::GrpcStatus> {
                #bind_params
                #call_unary
            }
        };

        syn::parse_quote! {
            #(#carried)*
            async fn #name(
                &self,
                request: ::tonic::Request<#request_arg_ty>,
            ) -> ::std::result::Result<::tonic::Response<#answer_ty>, ::tonic::Status> {
                #install_and_run
            }
        }
    };

    generated.push(syn::ImplItem::Fn(generated_fn));

    let mut handler = method.clone();
    handler.sig.ident = hidden;
    handler.attrs.retain(|attr| {
        !attr_is(attr, "grpc_method")
            && !attr_is(attr, "grpc_stream")
            && !has_enhancer_attribute(attr)
            && !attr_is(attr, "set_metadata")
    });

    Ok((handler, run_fn, generated))
}

/// The associated type tonic declares for a streaming method: the method's
/// name in Pascal case with `Stream` appended, which is the pairing
/// tonic-build creates from one proto identifier.
fn assoc_stream_ident(method: &syn::Ident) -> syn::Ident {
    let pascal: String = method
        .to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect();
    format_ident!("{}Stream", pascal)
}

/// The `Item` a returned `impl Stream<Item = …>` binds.
fn stream_item_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::ImplTrait(imp) = ty else {
        return None;
    };
    imp.bounds.iter().find_map(|bound| {
        let syn::TypeParamBound::Trait(bound) = bound else {
            return None;
        };
        let segment = bound.path.segments.last()?;
        if segment.ident != "Stream" {
            return None;
        }
        let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
            return None;
        };
        args.args.iter().find_map(|arg| match arg {
            syn::GenericArgument::AssocType(assoc) if assoc.ident == "Item" => {
                Some(assoc.ty.clone())
            }
            _ => None,
        })
    })
}

/// The `T` of a `Response<T>`, when a handler answers with the response itself.
fn response_inner_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Response" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(inner) => Some(inner.clone()),
        _ => None,
    })
}

/// The `T` of a `Result<T, _>` written as a type.
fn result_ok_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ok) => Some(ok.clone()),
        _ => None,
    })
}

/// What a handler answers with, and whether it can fail: `Result<T, E>` gives
/// `T`, and a bare `T` is the reply of a handler that cannot.
fn answer_type(output: &syn::ReturnType) -> Result<(syn::Type, bool)> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "a gRPC handler answers with its reply, or `Result<Reply, YourError>`",
        ));
    };

    Ok(match result_ok_type(ty) {
        Some(ok) => (ok, true),
        None => ((**ty).clone(), false),
    })
}

/// Build the wrapper's proto-trait method body. Runs the full pipeline
/// (guards → interceptors → user delegation) via `run_grpc_pipeline`, and
/// answers with what it produced: a `GrpcStatus` mapped to `tonic::Status`, or
/// the reply downcast back to this method's own type. The reply is erased for
/// the trip because the enhancers between the two ends are one list for the
/// whole service, and each method's reply is its own type.
///
/// Delegation uses UFCS — `<UserType as ProtoTrait>::method(&inner, ...)`
/// — so the user's body's `self.<field>` accesses, `Self::SomeStream`
/// associated-type references, and any inherent-helper calls resolve in
/// the user's original impl context, unchanged by this rewrite.
fn build_wrapper_method(
    method: &syn::ImplItemFn,
    self_ident: &syn::Ident,
    trait_short: &str,
    impl_attrs: &[syn::Attribute],
    shapes: &Path,
    trait_path: &Path,
    assoc_idents: &std::collections::HashSet<String>,
) -> Result<TokenStream> {
    let sig = &method.sig;
    let method_name_lit = sig.ident.to_string();
    let method_path_lit = format!("{}/{}", trait_short, method_name_lit);
    let marker = format_ident!("{}", to_upper_camel(&method_name_lit));
    let shape = quote! { #shapes::#marker };
    let run = run_ident(&sig.ident);

    // The impl block's `#[set_metadata]` entries then the method's, merged here rather than at every
    // call. The map is built once and shared, the service having one shape for the process.
    let merged = merged_metadata_exprs(impl_attrs, &method.attrs)?;
    let declared_metadata = match metadata_ctor(&merged) {
        Some(ctor) => quote! {
            static __DECLARED: ::std::sync::OnceLock<
                ::std::sync::Arc<::ulo::context::Metadata>
            > = ::std::sync::OnceLock::new();
            let __declared = ::std::option::Option::Some(
                __DECLARED.get_or_init(|| ::std::sync::Arc::new(#ctor)).clone(),
            );
        },
        None => quote! {
            let __declared: ::std::option::Option<
                ::std::sync::Arc<::ulo::context::Metadata>
            > = ::std::option::Option::None;
        },
    };

    // The first non-receiver argument is the tonic Request. Its metadata and
    // remote_addr come off a borrow, read before the request is installed on
    // the execution.
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
    let generics = &sig.generics;

    let output = &sig.output;
    let reply_ty = user_reply_type(output, self_ident, trait_path, assoc_idents)?;

    Ok(quote! {
        #asyncness fn #method_ident #generics (#inputs) #output {
            let (__ascii, __binary) = ::ulo_grpc::read_metadata(#req_ident.metadata());
            #declared_metadata
            // The path the caller dialled, which only the wire carries: an impl
            // block shows Rust names, no package, and a route casing that holds
            // by convention rather than by rule. The adapter puts it on the
            // request; a pipeline driven without one falls back to the names
            // this macro can see.
            let __method: ::std::string::String = #req_ident
                .extensions()
                .get::<::ulo::grpc::GrpcMethodPath>()
                .map(|__p| __p.as_str().to_string())
                .unwrap_or_else(|| #method_path_lit.to_string());
            let __ctx = ::ulo::grpc::GrpcContext::from_wire(
                __method,
                __ascii,
                __binary,
                #req_ident.remote_addr(),
                __declared,
            );
            ::ulo_grpc::arm_deadline(&__ctx);

            // The handler receives the tonic request, never the context, so the
            // context's extension bag rides the request to reach it. A handle,
            // not a copy — the guards below write into the same bag.
            let mut #req_ident = #req_ident;
            #req_ident.extensions_mut().insert(
                ::ulo::context::ExecutionContext::extensions(&__ctx).clone()
            );
            // The context itself rides the request too, since the signature
            // cannot carry it: this is where a handler reaches the cancellation
            // token, the declared metadata, and the execution's cache.
            #req_ident.extensions_mut().insert(__ctx.clone());
            // Installed before the guards run, so a guard reads a copy of the
            // message and the handler's extractor still takes the original.
            <#shape as ::ulo_grpc::MethodShape>::install(#req_ident, &__ctx);
            // Released when this future ends, returned or dropped: the installed request holds
            // this context, and nothing else breaks that cycle for a request nothing took.
            let __release = ::ulo::__grpc::ReleaseRequest(__ctx.clone());

            let __source = self.source.clone();
            let __build_ctx = __ctx.clone();
            let __run_ctx = __ctx.clone();

            let __pipeline = ::ulo::__grpc::run_grpc_pipeline(
                &__ctx,
                &self.enhancers,
                #method_name_lit,
                move || async move {
                    // The service is asked for here and nowhere earlier: a guard that rejects never
                    // builds one. Construction sits inside the same panic recovery as the handler
                    // body, so a panicking constructor renders a status rather than tearing down
                    // the connection.
                    let __caught = ::ulo::__grpc::catch_handler_panic(async move {
                        let __inner = match __source
                            .resolve(::ulo::di::Execution::Grpc(__build_ctx))
                            .await
                        {
                            ::std::result::Result::Ok(__inner) => __inner,
                            ::std::result::Result::Err(__failed) => {
                                return ::std::result::Result::Err(
                                    ::ulo::grpc::GrpcStatus::of(__failed),
                                );
                            }
                        };
                        #self_ident::#run(&__inner, &__run_ctx).await
                    }).await;
                    match __caught {
                        // Carried here and downcast below: the guards, interceptors and error
                        // handlers between the two are one list for the whole service, and this
                        // reply's type is this method's. The headers on it are not — they reach
                        // those enhancers through `ReplyEnvelope` without this type in hand.
                        ::std::result::Result::Ok(::std::result::Result::Ok(__reply)) => {
                            ::std::result::Result::Ok(::ulo_grpc::reply(__reply))
                        }
                        ::std::result::Result::Ok(::std::result::Result::Err(__status)) => {
                            ::std::result::Result::Err(__status)
                        }
                        ::std::result::Result::Err(__panic_event) => {
                            // The event rides on the status, so the chain is offered the type a
                            // `#[catch(PanicRecovered)]` handler matches rather than the message
                            // it renders as.
                            let __message = format!("handler panicked: {}", __panic_event);
                            ::std::result::Result::Err(
                                ::ulo::grpc::GrpcStatus::internal(__message)
                                    .caused_by(__panic_event),
                            )
                        }
                    }
                },
            ).await;

            match __pipeline {
                // Guards, interceptors, the handler and every panic below them all fail as one
                // `Err`, and the chain has already had its claim on it by the time it arrives.
                ::std::result::Result::Err(__status) => {
                    ::std::result::Result::Err(::ulo_grpc::to_status(__status))
                }
                ::std::result::Result::Ok(__reply) => {
                    match ::ulo::grpc::GrpcReply::downcast::<#reply_ty>(__reply) {
                        ::std::result::Result::Ok(__reply) => {
                            // The execution ends when the answer does. A streaming reply
                            // has produced nothing yet, so the context rides it to the
                            // last item instead of dying with the handler.
                            let (__meta, __body, __ext) = __reply.into_parts();
                            ::std::result::Result::Ok(::tonic::Response::from_parts(
                                __meta,
                                ::ulo::__grpc::IntoScoped::into_scoped(__body, __ctx.clone()),
                                __ext,
                            ))
                        }
                        // An enhancer answered with a reply of another method's type, or of no
                        // method's. Nothing below can render it, so the call fails naming both.
                        ::std::result::Result::Err(__other) => ::std::result::Result::Err(
                            ::tonic::Status::internal(format!(
                                "an enhancer answered this call with `{}`, and it replies `{}`",
                                ::ulo::grpc::GrpcReply::carries(&__other),
                                ::std::any::type_name::<#reply_ty>(),
                            )),
                        ),
                    }
                }
            }
        }
    })
}

/// The `tonic::Response<_>` the user's own impl answers with, which is what the wrapper has to
/// name to take a reply back out of a `GrpcReply`.
///
/// The wrapper's signature is the generated method's with `Self` rebound, so `Self::GreetManyStream`
/// there is the `ScopedGrpcStream<_>` the wrapper declares while the value in hand is the stream it
/// wraps. Rewriting each associated type through the user's impl names what was erased.
fn user_reply_type(
    output: &syn::ReturnType,
    self_ident: &syn::Ident,
    trait_path: &Path,
    assoc_idents: &std::collections::HashSet<String>,
) -> Result<syn::Type> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "a gRPC method answers with `Result<Response<_>, Status>`",
        ));
    };
    let mut reply = result_ok_type(ty).ok_or_else(|| {
        syn::Error::new_spanned(
            ty,
            "a gRPC method answers with `Result<Response<_>, Status>`",
        )
    })?;
    rewrite_self_assoc(&mut reply, self_ident, trait_path, assoc_idents);
    Ok(reply)
}

/// Rebind every `Self::X` in `ty` that names an associated type of this impl to
/// `<UserType as Trait>::X`.
fn rewrite_self_assoc(
    ty: &mut syn::Type,
    self_ident: &syn::Ident,
    trait_path: &Path,
    declared: &std::collections::HashSet<String>,
) {
    match ty {
        syn::Type::Path(tp) => {
            // `Self::X`, and `<Self as Trait>::X` which normalises to the same type.
            let assoc = if tp.qself.is_none() && tp.path.segments.len() == 2 {
                let head = &tp.path.segments[0];
                let tail = &tp.path.segments[1];
                (head.ident == "Self" && declared.contains(&tail.ident.to_string()))
                    .then(|| tail.ident.clone())
            } else if tp.qself.as_ref().is_some_and(|qself| {
                matches!(qself.ty.as_ref(), syn::Type::Path(inner)
                    if inner.qself.is_none() && inner.path.is_ident("Self"))
            }) {
                tp.path
                    .segments
                    .last()
                    .filter(|last| declared.contains(&last.ident.to_string()))
                    .map(|last| last.ident.clone())
            } else {
                None
            };
            if let Some(name) = assoc {
                *ty = syn::parse_quote! { <#self_ident as #trait_path>::#name };
                return;
            }
            for segment in &mut tp.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &mut segment.arguments {
                    for arg in &mut args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            rewrite_self_assoc(inner, self_ident, trait_path, declared);
                        }
                    }
                }
            }
        }
        syn::Type::Reference(r) => {
            rewrite_self_assoc(&mut r.elem, self_ident, trait_path, declared)
        }
        syn::Type::Paren(p) => rewrite_self_assoc(&mut p.elem, self_ident, trait_path, declared),
        syn::Type::Group(g) => rewrite_self_assoc(&mut g.elem, self_ident, trait_path, declared),
        syn::Type::Tuple(t) => {
            for inner in &mut t.elems {
                rewrite_self_assoc(inner, self_ident, trait_path, declared);
            }
        }
        _ => {}
    }
}

/// Record every `Self::X` in `ty` whose `X` names an associated type of this
/// impl block, descending through the generic arguments of `Result<_, _>`,
/// `Response<_>` and anything else wrapping it.
fn collect_self_assoc(
    ty: &syn::Type,
    declared: &std::collections::HashSet<String>,
    found: &mut std::collections::HashSet<String>,
) {
    match ty {
        syn::Type::Path(tp) => {
            // `Self::X` — two segments, the first being `Self`.
            if tp.qself.is_none() && tp.path.segments.len() == 2 {
                let head = &tp.path.segments[0];
                let tail = &tp.path.segments[1];
                if head.ident == "Self" && declared.contains(&tail.ident.to_string()) {
                    found.insert(tail.ident.to_string());
                }
            }
            // `<Self as Trait>::X`, which normalises to the same type.
            if let Some(qself) = &tp.qself {
                if matches!(qself.ty.as_ref(), syn::Type::Path(inner)
                    if inner.qself.is_none() && inner.path.is_ident("Self"))
                {
                    if let Some(last) = tp.path.segments.last() {
                        if declared.contains(&last.ident.to_string()) {
                            found.insert(last.ident.to_string());
                        }
                    }
                }
            }
            for segment in &tp.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            collect_self_assoc(inner, declared, found);
                        }
                    }
                }
            }
        }
        syn::Type::Reference(r) => collect_self_assoc(&r.elem, declared, found),
        syn::Type::Paren(p) => collect_self_assoc(&p.elem, declared, found),
        syn::Type::Group(g) => collect_self_assoc(&g.elem, declared, found),
        syn::Type::Tuple(t) => {
            for inner in &t.elems {
                collect_self_assoc(inner, declared, found);
            }
        }
        _ => {}
    }
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

#[cfg(test)]
mod naming_tests {
    use super::*;

    /// The table ulo-build tests against; a divergence here is a module the
    /// macro names and ulo-build never wrote.
    #[test]
    fn the_shapes_module_is_named_as_ulo_build_names_it() {
        for (input, expected) in [
            ("Service", "service"),
            ("ThatHasALongName", "that_has_a_long_name"),
            ("greeter", "greeter"),
            ("ABCServiceX", "a_b_c_service_x"),
        ] {
            assert_eq!(to_snake(input), expected);
        }
        assert_eq!(to_upper_camel("greet"), "Greet");
        assert_eq!(to_upper_camel("greet_all"), "GreetAll");
        assert_eq!(to_upper_camel("r#type"), "Type");
    }

    #[test]
    fn the_shapes_path_sits_beside_the_trait_s_module() {
        let trait_path: Path = syn::parse_quote!(pb::greeter_server::Greeter);
        let shapes = infer_shapes_path(&trait_path);
        assert_eq!(
            quote!(#shapes).to_string(),
            quote!(pb::greeter_ulo).to_string()
        );

        let bare: Path = syn::parse_quote!(watcher_server::Watcher);
        let beside = infer_shapes_path(&bare);
        assert_eq!(quote!(#beside).to_string(), quote!(watcher_ulo).to_string());
    }
}
