//! `#[message("pattern")]`: one `Rpc` request-reply handler over `ulo-handler-codegen`, and the
//! expansion `#[event]` shares.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, and the three items the
//! `__handler` protocol owes (`ulo_handler_codegen::emit::MountFn`), the handler value being
//!
//! ```text
//! ::ulo_rpc::__private::RpcHandler::new("invoices.create", ::ulo_rpc::__private::Kind::Message, __ulo_call)
//!     .payload((&&&::ulo_rpc::__private::PayloadKindProbe::<P0>::new()).kind())
//!     /* one per parameter */
//! ```
//!
//! and the shape `::ulo_rpc::__private::shape(false || (&&InboundProbe::<P0>::new()).streams() || .., (&&&__ulo_reply).streams())`,
//! the request side probed at each parameter's type and the reply side at the type the handler
//! returns, through `ReplyShapeProbe` over a closure that names the handler's call and is never
//! run, so an alias or an opaque type reads as the type it stands for. The generated call names
//! `::ulo_rpc::__private` for `Param`, `controller` and the reply probe, `ulo-rpc`'s own (see that
//! module).

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Ident, ImplItemFn, LitStr};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::params::{self, HandlerSig, Receiver};
use ulo_handler_codegen::{Paths, protocol, reply};

/// The transport's key, which equals `<ulo_rpc::Rpc as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "rpc";

/// Which attribute is expanding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Message,
    Event,
}

impl Kind {
    fn attr_name(self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::Event => "event",
        }
    }
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    expand_kind(Kind::Message, attr, item)
}

pub(crate) fn expand_kind(kind: Kind, attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attr_name = kind.attr_name();
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            format!("#[{attr_name}] takes the pattern, as in #[{attr_name}(\"invoices.create\")]"),
        ));
    }
    let pattern: LitStr = syn::parse2(attr)?;
    if let Err(reason) = check_pattern(&pattern.value()) {
        return Err(syn::Error::new(pattern.span(), format!("invalid pattern: {reason}")));
    }
    let mut item: ImplItemFn = syn::parse2(item)?;
    let Some(tokens) = protocol::take_handler_attr(&mut item.attrs)? else {
        return Err(protocol::outside_routes(attr_name));
    };
    let sig = params::analyze(&item.sig)?;
    let output_span = match &item.sig.output {
        syn::ReturnType::Default => item.sig.ident.span(),
        syn::ReturnType::Type(_, ty) => ty.span(),
    };
    reply::rewrite_opaque_returns(&mut item.sig);

    let paths = rpc_paths();
    let mount = MountFn {
        tokens: &tokens,
        sig: &sig,
        key: KEY,
        paths: &paths,
        handler_value: handler_value(kind, &pattern, &sig, &paths),
        route: Some(quote!(#pattern)),
        shape: Some(shape(kind, &sig, output_span, &paths)),
    }
    .emit();

    Ok(quote! {
        #item
        #mount
    })
}

/// `ulo-rpc`'s paths, `transport` naming `ulo-rpc` itself: the generated call reads `Param` and
/// `controller` through `::ulo_rpc::__private`, which re-exports them from `ulo-transport`, and
/// takes the reply probe there too.
fn rpc_paths() -> Paths {
    Paths { core: quote!(::ulo), transport: quote!(::ulo_rpc), this: quote!(::ulo_rpc), marker: quote!(::ulo_rpc::Rpc) }
}

/// A pattern is a subject, channel, queue or topic name: non-empty, without whitespace or control
/// characters, which no broker accepts in one.
fn check_pattern(pattern: &str) -> Result<(), &'static str> {
    if pattern.is_empty() {
        return Err("a pattern is not empty");
    }
    if pattern.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a pattern holds no whitespace or control character");
    }
    Ok(())
}

/// The `RpcHandler` the mount function hands `HandlerSpec::new`: the pattern, the kind, the call
/// closure, and one payload-kind probe per parameter, spanned at its type.
fn handler_value(kind: Kind, pattern: &LitStr, sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let call = emit::call_ident();
    let kind = match kind {
        Kind::Message => quote!(#this::__private::Kind::Message),
        Kind::Event => quote!(#this::__private::Kind::Event),
    };
    let probes = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            .payload((&&&#this::__private::PayloadKindProbe::<#ty>::new()).kind())
        }
    });
    quote! {
        {
            #[allow(unused_imports)]
            use #this::__private::{NotPayload as _, ViaBinary as _, ViaSerde as _};
            #this::__private::RpcHandler::new(#pattern, #kind, #call)
                #(#probes)*
        }
    }
}

/// `.shape(..)`'s argument: the request side from one `Inbound<T>` probe per parameter, the reply
/// side from `ReplyShapeProbe` over the handler's output type. For `#[event]` the block first
/// asserts the output is no stream, spanned at the return type: the stream arms of `event_reply`
/// answer `StreamedReplyOnEvent`, which `let (): ()` refuses.
fn shape(kind: Kind, sig: &HandlerSig, output_span: Span, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let probes = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            || (&&#this::__private::InboundProbe::<#ty>::new()).streams()
        }
    });
    let reply = Ident::new("__ulo_reply", Span::mixed_site());
    let probe = reply_probe(sig, &reply, paths);
    let event_check = (kind == Kind::Event).then(|| {
        quote_spanned! {output_span=>
            let (): () = (&&&#reply).event_reply();
        }
    });
    let streams = quote_spanned! {output_span=> (&&&#reply).streams() };
    quote! {
        {
            #[allow(unused_imports)]
            use #this::__private::{NotInbound as _, StreamsBare as _, StreamsInResult as _, StreamsNot as _, ViaInbound as _};
            #probe
            #event_check
            #this::__private::shape(false #(#probes)*, #streams)
        }
    }
}

/// `let <reply> = ReplyShapeProbe::of(&|| async move { .. });`, the closure calling the handler
/// with the receiver and every parameter bound to `unreachable!()`, so its future's output is the
/// handler's output type. The closure is never called; it exists for its type. The `allow`s cover
/// what the `unreachable!()` bindings trip in the user's crate: rustc's `unreachable_code` and
/// clippy's `diverging_sub_expression`.
fn reply_probe(sig: &HandlerSig, reply: &Ident, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let receiver = Ident::new("__ulo_this", Span::mixed_site());
    let receiver_ty = match sig.receiver {
        Receiver::Ref => quote!(&Self),
        Receiver::Arc => quote!(::std::sync::Arc<Self>),
    };
    let args: Vec<Ident> =
        sig.params.iter().map(|param| Ident::new(&format!("__ulo_arg{}", param.index), Span::mixed_site())).collect();
    let bindings = sig.params.iter().zip(&args).map(|(param, arg)| {
        let ty = &param.ty;
        quote_spanned! {param.span=> let #arg: #ty = ::core::unreachable!(); }
    });
    let method = &sig.ident;
    let awaited = sig.is_async.then(|| quote!(.await));
    quote! {
        #[allow(unreachable_code, clippy::diverging_sub_expression)]
        let #reply = #this::__private::ReplyShapeProbe::of(&|| async move {
            let #receiver: #receiver_ty = ::core::unreachable!();
            #(#bindings)*
            Self::#method(#receiver, #(#args),*) #awaited
        });
    }
}
