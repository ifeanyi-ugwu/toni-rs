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
//! and the shape `::ulo_rpc::__private::shape(false || (&&InboundProbe::<P0>::new()).streams() || .., <reply>)`,
//! the request side probed at each parameter's type and the reply side read from the return type
//! as written. The generated call names `::ulo_rpc::__private` for `Param`, `controller` and the
//! reply probe, `ulo-rpc`'s own (see that module).

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{ImplItemFn, LitStr, ReturnType, Type, TypeParamBound};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::params::{self, HandlerSig};
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
    let streams_reply = returns_stream(&item.sig.output);
    if kind == Kind::Event && streams_reply {
        return Err(syn::Error::new_spanned(
            &item.sig.output,
            "an `#[event]` handler answers nothing; a stream return makes it a streamed reply, which `#[message]` declares",
        ));
    }
    reply::rewrite_opaque_returns(&mut item.sig);

    let paths = rpc_paths();
    let mount = MountFn {
        tokens: &tokens,
        sig: &sig,
        key: KEY,
        paths: &paths,
        handler_value: handler_value(kind, &pattern, &sig, &paths),
        route: Some(quote!(#pattern)),
        shape: Some(shape(&sig, streams_reply, &paths)),
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
/// side as the return type reads.
fn shape(sig: &HandlerSig, streams_reply: bool, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let probes = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            || (&&#this::__private::InboundProbe::<#ty>::new()).streams()
        }
    });
    quote! {
        {
            #[allow(unused_imports)]
            use #this::__private::{NotInbound as _, ViaInbound as _};
            #this::__private::shape(false #(#probes)*, #streams_reply)
        }
    }
}

/// Whether the return type, as written, is a stream: an `impl` or `dyn` bound naming `Stream` or
/// `TryStream`, or a path ending in `BoxStream` or `LocalBoxStream`, anywhere in it, a `Result`'s
/// `Ok` side included. Read from the spelling, since an opaque return type cannot be named where
/// the shape is recorded; a stream behind an alias is declared unary, refused at runtime on a link
/// carrying no streamed reply, and answered as a stream on every other.
fn returns_stream(output: &ReturnType) -> bool {
    match output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => names_stream(ty),
    }
}

fn names_stream(ty: &Type) -> bool {
    match ty {
        Type::ImplTrait(opaque) => bounds_name_stream(opaque.bounds.iter()),
        Type::TraitObject(object) => bounds_name_stream(object.bounds.iter()),
        Type::Paren(inner) => names_stream(&inner.elem),
        Type::Group(inner) => names_stream(&inner.elem),
        Type::Reference(reference) => names_stream(&reference.elem),
        Type::Tuple(tuple) => tuple.elems.iter().any(names_stream),
        Type::Path(path) => path.path.segments.iter().any(|segment| {
            segment.ident == "BoxStream" || segment.ident == "LocalBoxStream" || arguments_name_stream(&segment.arguments)
        }),
        _ => false,
    }
}

fn bounds_name_stream<'a>(mut bounds: impl Iterator<Item = &'a TypeParamBound>) -> bool {
    bounds.any(|bound| match bound {
        TypeParamBound::Trait(bound) => bound.path.segments.iter().any(|segment| {
            segment.ident == "Stream" || segment.ident == "TryStream" || arguments_name_stream(&segment.arguments)
        }),
        _ => false,
    })
}

fn arguments_name_stream(arguments: &syn::PathArguments) -> bool {
    let syn::PathArguments::AngleBracketed(arguments) = arguments else { return false };
    arguments.args.iter().any(|argument| match argument {
        syn::GenericArgument::Type(ty) => names_stream(ty),
        syn::GenericArgument::AssocType(assoc) => names_stream(&assoc.ty),
        _ => false,
    })
}
