//! `#[method(Marker)]`: one `Grpc` handler over `ulo-handler-codegen`, its path and shape read from
//! the marker and checked against the signature.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, a hidden method taking the same
//! parameters, and the three items the `__handler` protocol owes, whose call reads the hidden
//! method. For `#[method(pb::user_service::GetUser)] async fn get_user(&self, req: Message<pb::GetUserRequest>)
//! -> Result<pb::User, UserError>` the hidden method is, in shape:
//!
//! ```text
//! #[doc(hidden)]
//! async fn __ulo_grpc_get_user(&self, __ulo_p0: Message<pb::GetUserRequest>)
//!     -> Result<::ulo_grpc::__private::Answer, ::ulo::BoxError>
//! {
//!     (&&ParamProbe::<pb::user_service::GetUser, Message<pb::GetUserRequest>>::new())
//!         .body::<{ streams_request(<pb::user_service::GetUser as Method>::SHAPE) }>().matches();
//!     let __ulo_out = Self::get_user(self, __ulo_p0).await;
//!     (&&&&&&ReplyProbe::<pb::user_service::GetUser, _>::new(__ulo_out))
//!         .answer::<{ streams_reply(<pb::user_service::GetUser as Method>::SHAPE) }>().checked()
//! }
//! ```
//!
//! with the probes' traits imported anonymously and `ulo_grpc::__private` written in full. The
//! handler value is `GrpcHandler::new::<Marker, _>(call)`, the route `Marker::PATH` and the shape
//! `Marker::SHAPE`. A generic handler gets no hidden method: `MountFn::emit` refuses it.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Ident, ImplItemFn, Type};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::params::{self, HandlerSig, Receiver};
use ulo_handler_codegen::{Paths, protocol, reply};

/// The transport's key, which equals `<ulo_grpc::Grpc as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "grpc";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[method] takes the method marker `ulo-build` generated, as in #[method(pb::user_service::GetUser)]",
        ));
    }
    let marker: Type = syn::parse2(attr)?;
    let mut item: ImplItemFn = syn::parse2(item)?;
    let Some(tokens) = protocol::take_handler_attr(&mut item.attrs)? else {
        return Err(protocol::outside_routes("method"));
    };
    let sig = params::analyze(&item.sig)?;
    reply::rewrite_opaque_returns(&mut item.sig);

    let (hidden, called) = if sig.is_generic {
        (TokenStream::new(), sig)
    } else {
        let ident = format_ident!("__ulo_grpc_{}", tokens.name(), span = sig.ident.span());
        let hidden = hidden_method(&item, &sig, &marker, &ident);
        let called = HandlerSig { ident, receiver: sig.receiver, params: sig.params, is_async: true, is_generic: false };
        (hidden, called)
    };

    let paths = Paths::new("ulo_grpc", "Grpc");
    let call = emit::call_ident();
    let mount = MountFn {
        tokens: &tokens,
        sig: &called,
        key: KEY,
        paths: &paths,
        handler_value: quote!(::ulo_grpc::__private::GrpcHandler::new::<#marker, _>(#call)),
        route: Some(quote!(<#marker as ::ulo_grpc::Method>::PATH)),
        shape: Some(quote!(<#marker as ::ulo_grpc::Method>::SHAPE)),
    }
    .emit();

    Ok(quote! {
        #item
        #hidden
        #mount
    })
}

/// The hidden method the generated call reads: the handler's parameters, each checked against the
/// marker at its type, the handler called, and its output turned into an `Answer` by the reply
/// probe, checked against the marker at the return type. It carries the handler's `#[cfg]`s, so it
/// exists where the handler does.
fn hidden_method(item: &ImplItemFn, sig: &HandlerSig, marker: &Type, ident: &Ident) -> TokenStream {
    let cfgs = item.attrs.iter().filter(|attr| attr.path().is_ident("cfg"));
    let receiver = match sig.receiver {
        Receiver::Ref => quote!(&self),
        Receiver::Arc => quote!(self: ::std::sync::Arc<Self>),
    };
    let args: Vec<Ident> = sig.params.iter().map(|param| Ident::new(&format!("__ulo_p{}", param.index), Span::mixed_site())).collect();
    let types: Vec<&Type> = sig.params.iter().map(|param| &param.ty).collect();
    let checks = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            (&&::ulo_grpc::__private::ParamProbe::<#marker, #ty>::new())
                .body::<{ ::ulo_grpc::__private::streams_request(<#marker as ::ulo_grpc::Method>::SHAPE) }>()
                .matches();
        }
    });
    let method = &sig.ident;
    let awaited = sig.is_async.then(|| quote!(.await));
    let out = Ident::new("__ulo_out", Span::mixed_site());
    let reply = quote_spanned! {item.sig.output.span()=>
        (&&&&&&::ulo_grpc::__private::ReplyProbe::<#marker, _>::new(#out))
            .answer::<{ ::ulo_grpc::__private::streams_reply(<#marker as ::ulo_grpc::Method>::SHAPE) }>()
            .checked()
    };
    quote! {
        #(#cfgs)*
        #[doc(hidden)]
        async fn #ident(#receiver, #(#args: #types),*)
            -> ::core::result::Result<::ulo_grpc::__private::Answer, ::ulo::BoxError>
        {
            {
                #[allow(unused_imports)]
                use ::ulo_grpc::__private::{NotBody as _, ViaBody as _};
                #(#checks)*
            }
            let #out = Self::#method(self, #(#args),*) #awaited;
            #[allow(unused_imports)]
            use ::ulo_grpc::__private::{
                StreamValue as _, StreamViaBoxError as _, StreamViaCallError as _, Value as _, ValueViaBoxError as _,
                ValueViaCallError as _,
            };
            #reply
        }
    }
}
