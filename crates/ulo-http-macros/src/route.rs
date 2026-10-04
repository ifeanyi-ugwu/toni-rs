//! One HTTP handler attribute over `ulo-handler-codegen`.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, and the three items the
//! `__handler` protocol owes (`ulo_handler_codegen::emit::MountFn`), the handler value being
//!
//! ```text
//! ::ulo_http::__private::HttpHandler::new(::ulo_http::__private::Method::GET, "/users/{id}", __ulo_call)
//!     .path_check((&&::ulo_http::__private::PathProbe::<P0>::new()).check())
//!     .host_read((&&::ulo_http::__private::HostProbe::<P0>::new()).read())
//!     /* one pair per parameter */
//! ```
//!
//! with `ViaPath`, `NotPath`, `ViaHost` and `NotHost` imported anonymously, and the route
//! `"/users/{id}"` as `HandlerSpec::route`.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{Ident, ImplItemFn, LitStr};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::params::{self, HandlerSig};
use ulo_handler_codegen::{Paths, protocol, reply};

/// The transport's key, which equals `<ulo_http::Http as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "http";

/// `#[<method>("<pattern>")]` on `item`. `method` is the HTTP method's name as `http::Method`'s
/// associated constant spells it.
pub(crate) fn expand(method: &'static str, attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attr_name = method.to_ascii_lowercase();
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            format!("#[{attr_name}] takes the route pattern, as in #[{attr_name}(\"/users/{{id}}\")]"),
        ));
    }
    let pattern: LitStr = syn::parse2(attr)?;
    if let Err(reason) = crate::pattern::check(&pattern.value()) {
        return Err(syn::Error::new(pattern.span(), format!("invalid route pattern: {reason}")));
    }
    let mut item: ImplItemFn = syn::parse2(item)?;
    let Some(tokens) = protocol::take_handler_attr(&mut item.attrs)? else {
        return Err(protocol::outside_routes(&attr_name));
    };
    let sig = params::analyze(&item.sig)?;
    reply::rewrite_opaque_returns(&mut item.sig);

    let paths = Paths::new("ulo_http", "Http");
    let mount = MountFn {
        tokens: &tokens,
        sig: &sig,
        key: KEY,
        paths: &paths,
        handler_value: handler_value(method, &pattern, &sig, &paths),
        route: Some(quote!(#pattern)),
        shape: None,
    }
    .emit();

    Ok(quote! {
        #item
        #mount
    })
}

/// The `HttpHandler` the mount function hands `HandlerSpec::new`: the method, the pattern as
/// written, the call closure, and one `Path<T>` probe and one `Host<T>` probe per parameter,
/// spanned at its type. A generic handler never reaches this value: `MountFn::emit` refuses it.
fn handler_value(method: &str, pattern: &LitStr, sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let method = Ident::new(method, Span::call_site());
    let call = emit::call_ident();
    let checks = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            .path_check((&&#this::__private::PathProbe::<#ty>::new()).check())
            .host_read((&&#this::__private::HostProbe::<#ty>::new()).read())
        }
    });
    quote! {
        {
            #[allow(unused_imports)]
            use #this::__private::{NotHost as _, NotPath as _, ViaHost as _, ViaPath as _};
            #this::__private::HttpHandler::new(#this::__private::Method::#method, #pattern, #call)
                #(#checks)*
        }
    }
}
