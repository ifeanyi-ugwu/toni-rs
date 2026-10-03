//! The items a transport attribute owes `#[routes]` for one handler (see [`crate::protocol`]).
//!
//! For `#[ulo_http::get("/users/{id}")] async fn get(&self, id: Path<u64>, user: Ext<CurrentUser>)
//! -> Result<Json<User>, UserError>` inside a `#[routes]` impl with one impl-level
//! `#[interceptors(value = Timing::new())]`, the expansion is, in shape:
//!
//! ```text
//! async fn get(&self, id: Path<u64>, user: Ext<CurrentUser>) -> Result<Json<User>, UserError> { .. }
//!
//! #[doc(hidden)] #[allow(non_upper_case_globals)]
//! const __ULO_KEY_get: &'static str = <::ulo_http::Http as ::ulo::Transport>::KEY;
//!
//! #[doc(hidden)] #[allow(non_upper_case_globals)]
//! const __ULO_CHECKS_get: () = { assert!(!(<Path<u64> as P<Http, _>>::CONSUMES_BODY && ..), "..") };
//!
//! #[doc(hidden)]
//! fn __ulo_mount_get<__UloV0: ::ulo::Interceptor<::ulo_http::Http>>(
//!     m: &mut ::ulo::Mount<'_>,
//!     __ulo_shared: &::ulo::__private::Shared<(::ulo::__private::Arc<__UloV0>,)>,
//! ) {
//!     let (controller, method) = ::ulo::__private::__enhancer_specs!(::ulo_http::Http, "http", __ulo_shared, <tokens>);
//!     let mut meta = ::ulo::Metadata::new(); /* meta.controller(..); meta.method(..); */
//!     let mut dependencies = ::ulo::Dependencies::default();
//!     <Path<u64> as P<Http, _>>::dependencies(&mut dependencies); /* one per parameter */
//!     let handler = <transport's handler value, its call closure built from `call_body`>;
//!     m.handler(::ulo::HandlerSpec::new("get", handler)
//!         .controller(controller).method(method).meta(meta).route(..).shape(..).dependencies(dependencies));
//! }
//! ```
//!
//! `P` is `<transport>::__private::Param`. The call closure's body, from [`call_body`]:
//!
//! ```text
//! let __ulo_arg0: Path<u64> = <Path<u64> as P<Http, _>>::extract(&cx).await?;
//! let __ulo_arg1: Ext<CurrentUser> = <Ext<CurrentUser> as P<Http, _>>::extract(&cx).await?;
//! let __ulo_this = <transport>::__private::controller::<Self, Http>(&cx).await?;
//! let __ulo_out = Self::get(&*__ulo_this, __ulo_arg0, __ulo_arg1).await;
//! (&&&<transport>::__private::IntoReplyProbe::<Http, _>::new(__ulo_out)).into_reply(&cx)
//! ```
//!
//! Parameters extract in order, inside `dispatch`'s call, so after every guard admits. A
//! `self: Arc<Self>` receiver is passed `::ulo::Dep::into_arc(__ulo_this)`.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Ident, LitStr};

use crate::params::{HandlerSig, Receiver};
use crate::paths::Paths;
use crate::protocol::{HandlerTokens, MetaExpr, MetaTokens};
use crate::{body, keys, reply, shared};

/// One handler's three owed items.
pub struct MountFn<'a> {
    pub tokens: &'a HandlerTokens,
    pub sig: &'a HandlerSig,
    /// The transport's key as a literal, which `__enhancer_specs!` filters scoped entries by. It
    /// equals `<Marker as Transport>::KEY`; the transport crate keeps the two in step.
    pub key: &'a str,
    pub paths: &'a Paths,
    /// An expression evaluating to the transport's handler value, the `H` of `HandlerSpec<T, H>`.
    /// It may name [`call_ident`], bound by the mount function to the call closure, and
    /// [`mount_param_ident`], the mount function's `&mut Mount<'_>`: a WebSocket message handler's
    /// value is a block that first calls `<m>.once::<Self>(..)` to mount its gateway's connect
    /// handler (transports DESIGN §4.1, X15).
    pub handler_value: TokenStream,
    /// `.route(..)`'s argument, when the transport has one.
    pub route: Option<TokenStream>,
    /// `.shape(..)`'s argument, when the handler's shape is not `Unary`.
    pub shape: Option<TokenStream>,
}

impl MountFn<'_> {
    /// `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>`, as impl items.
    ///
    /// A handler with a type or const parameter gets a `compile_error!` spanned at its name in
    /// place of a working mount function: the call names each parameter's type inside the mount
    /// function, where the method's own parameters are not in scope. The three items are still
    /// written, empty, so `#[routes]`'s reads of them resolve and the refusal is the one error.
    pub fn emit(&self) -> TokenStream {
        let name = self.tokens.name();
        let handler = &self.tokens.handler;
        let span = handler.span();
        let core = &self.paths.core;
        let marker = &self.paths.marker;
        let key_const = keys::key_const(&name, span, self.paths);
        let checks_const = keys::checks_const_ident(&name, span);
        let mount_fn = mount_fn_ident(&name, span);
        let values = shared::shared_values(&self.tokens.controller);
        let (generics, shared_ty) = shared::mount_generics(&values, self.key, handler, self.paths);
        let shared = shared::shared_ident();
        let m = mount_param_ident();

        if self.sig.is_generic {
            let message = format!(
                "`{name}` declares a type or const parameter; a handler is not generic, since the generated call \
                 names each parameter's type outside the method"
            );
            return quote_spanned! {span=>
                #key_const

                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #checks_const: () = ();

                #[doc(hidden)]
                fn #mount_fn #generics(#m: &mut #core::Mount<'_>, #shared: #shared_ty) {
                    let _ = (#m, #shared);
                }

                ::core::compile_error!(#message);
            };
        }

        let checks = body::consumer_checks(&self.sig.params, self.paths);
        let key = LitStr::new(self.key, Span::call_site());
        let tokens = self.tokens;
        let call = call_ident();
        let call_closure = call_closure(self.sig, self.paths);
        let handler_value = &self.handler_value;
        let meta = metadata(&self.tokens.meta);
        let dependencies = dependencies(self.sig, self.paths);
        let route = self.route.as_ref().map(|route| quote!(.route(#route)));
        let shape = self.shape.as_ref().map(|shape| quote!(.shape(#shape)));
        let name_literal = LitStr::new(&name, span);
        let [controller, method, value] =
            ["__ulo_controller", "__ulo_method", "__ulo_handler"].map(|local| Ident::new(local, Span::mixed_site()));

        quote! {
            #key_const

            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            const #checks_const: () = #checks;

            #[doc(hidden)]
            fn #mount_fn #generics(#m: &mut #core::Mount<'_>, #shared: #shared_ty) {
                let _ = #shared;
                let (#controller, #method) = #core::__private::__enhancer_specs!(#marker, #key, #shared, #tokens);
                let #call = #call_closure;
                let #value = #handler_value;
                #m.handler(
                    #core::HandlerSpec::<#marker, _>::new(#name_literal, #value)
                        .controller(#controller)
                        .method(#method)
                        .meta(#meta)
                        #route
                        #shape
                        .dependencies(#dependencies),
                );
            }
        }
    }
}

/// `__ulo_call`, the binding of the call closure inside the mount function.
pub fn call_ident() -> Ident {
    Ident::new("__ulo_call", Span::mixed_site())
}

/// `__ulo_m`, the mount function's `&mut ::ulo::Mount<'_>` parameter.
pub fn mount_param_ident() -> Ident {
    Ident::new("__ulo_m", Span::mixed_site())
}

/// `__ulo_mount_<name>`.
pub fn mount_fn_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ulo_mount_{name}"), span)
}

/// `move |cx: <Marker as Transport>::Cx| -> ::ulo::BoxFuture<'static, Result<Reply, ::ulo::BoxError>>
/// { Box::pin(async move { <call_body> }) }`, cloneable into the transport's handler value.
pub fn call_closure(sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let core = &paths.core;
    let marker = &paths.marker;
    let cx = Ident::new("__ulo_cx", Span::mixed_site());
    let body = call_body(sig, &cx, paths);
    quote! {
        move |#cx: <#marker as #core::Transport>::Cx| -> #core::BoxFuture<
            'static,
            ::core::result::Result<<#marker as #core::Transport>::Reply, #core::BoxError>,
        > {
            ::std::boxed::Box::pin(async move { #body })
        }
    }
}

/// The statements of the call: extract each parameter, resolve the controller, call the method,
/// probe the reply. `cx` names the context binding.
pub fn call_body(sig: &HandlerSig, cx: &Ident, paths: &Paths) -> TokenStream {
    let core = &paths.core;
    let transport = &paths.transport;
    let marker = &paths.marker;
    let args: Vec<Ident> =
        sig.params.iter().map(|param| Ident::new(&format!("__ulo_arg{}", param.index), Span::mixed_site())).collect();
    let extractions = sig.params.iter().zip(&args).map(|(param, arg)| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            let #arg: #ty = <#ty as #transport::__private::Param<#marker, _>>::extract(&#cx).await?;
        }
    });
    let this = Ident::new("__ulo_this", Span::mixed_site());
    let out = Ident::new("__ulo_out", Span::mixed_site());
    let receiver = match sig.receiver {
        Receiver::Ref => quote!(&*#this),
        Receiver::Arc => quote!(#core::Dep::into_arc(#this)),
    };
    let method = &sig.ident;
    let awaited = sig.is_async.then(|| quote!(.await));
    let call = quote_spanned! {method.span()=>
        Self::#method(#receiver, #(#args),*) #awaited
    };
    let reply = reply::reply_call(&out, cx, paths);
    quote! {
        #(#extractions)*
        let #this = #transport::__private::controller::<Self, #marker>(&#cx).await?;
        let #out = #call;
        #reply
    }
}

/// `let mut dependencies = ::ulo::Dependencies::default();` followed by each parameter's
/// `<Ty as Param<Marker, _>>::dependencies(&mut dependencies);`, evaluating to `dependencies`.
pub fn dependencies(sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let core = &paths.core;
    let transport = &paths.transport;
    let marker = &paths.marker;
    let dependencies = Ident::new("__ulo_dependencies", Span::mixed_site());
    let reads = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            <#ty as #transport::__private::Param<#marker, _>>::dependencies(&mut #dependencies);
        }
    });
    quote! {
        {
            #[allow(unused_mut)]
            let mut #dependencies = <#core::Dependencies as ::core::default::Default>::default();
            #(#reads)*
            #dependencies
        }
    }
}

/// A block evaluating to the handler's `::ulo::Metadata`: `controller(expr)` per impl-level
/// declaration and `method(expr)` per method-level one, each spanned at its expression and
/// compiled under its gates, built once when the handler mounts.
pub fn metadata(meta: &MetaTokens) -> TokenStream {
    let metadata = Ident::new("__ulo_meta", Span::mixed_site());
    let declare = |tier: &str, declared: &MetaExpr| {
        let MetaExpr { gates, expr } = declared;
        let tier = Ident::new(tier, expr.span());
        quote_spanned! {expr.span()=>
            #(#gates)*
            #metadata.#tier(#expr);
        }
    };
    let controller = meta.controller.iter().map(|declared| declare("controller", declared));
    let method = meta.method.iter().map(|declared| declare("method", declared));
    quote! {
        {
            #[allow(unused_mut)]
            let mut #metadata = ::ulo::Metadata::new();
            #(#controller)*
            #(#method)*
            #metadata
        }
    }
}
