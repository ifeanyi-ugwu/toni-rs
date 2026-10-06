//! `#[message("event")]`: one `Ws` handler over `ulo-handler-codegen`, whose handler value calls
//! `<Self as GatewayConfig>::mount_gateway` through the mount function's `Mount`.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, and the three items the
//! `__handler` protocol owes, the handler value being
//!
//! ```text
//! {
//!     __ulo_m.once::<Self>(<Self as ::ulo_ws::GatewayConfig>::mount_gateway);
//!     ::ulo_ws::__private::WsHandler::new("chat.send", __ulo_call)
//! }
//! ```
//!
//! and the event as `HandlerSpec::route`. The generated call reaches `Param`, `controller` and the
//! reply probe through `::ulo_ws::__private::reply`, whose probe answers `()`, any `Serialize`
//! value, a `Frame` and a stream, which `ulo-transport`'s `IntoReply`-only probe cannot.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{ImplItemFn, LitStr};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::{Paths, params, protocol, reply};

/// The transport's key, which equals `<ulo_ws::Ws as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "ws";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if attr.is_empty() {
        return Err(syn::Error::new(Span::call_site(), "#[message] takes the event, as in #[message(\"chat.send\")]"));
    }
    let event: LitStr = syn::parse2(attr)?;
    match event.value().as_str() {
        "" => return Err(syn::Error::new(event.span(), "an event name is not empty")),
        "cancel" => {
            return Err(syn::Error::new(
                event.span(),
                "`cancel` is the envelope's reserved event: a client sends `{\"event\":\"cancel\",\"id\":..}` to cancel a message",
            ));
        }
        _ => {}
    }
    let mut item: ImplItemFn = syn::parse2(item)?;
    let Some(tokens) = protocol::take_handler_attr(&mut item.attrs)? else {
        return Err(protocol::outside_routes("message"));
    };
    let sig = params::analyze(&item.sig)?;
    reply::rewrite_opaque_returns(&mut item.sig);

    let paths = Paths {
        core: quote!(::ulo),
        transport: quote!(::ulo_ws::__private::reply),
        this: quote!(::ulo_ws),
        marker: quote!(::ulo_ws::Ws),
    };
    let mount = MountFn {
        tokens: &tokens,
        sig: &sig,
        key: KEY,
        paths: &paths,
        handler_value: handler_value(&event),
        route: Some(quote!(#event)),
        shape: None,
    }
    .emit();

    Ok(quote! {
        #item
        #mount
    })
}

/// The gateway's connect handler mounted once per controller type, then the `WsHandler`.
fn handler_value(event: &LitStr) -> TokenStream {
    let m = emit::mount_param_ident();
    let call = emit::call_ident();
    quote! {
        {
            #m.once::<Self>(<Self as ::ulo_ws::GatewayConfig>::mount_gateway);
            ::ulo_ws::__private::WsHandler::new(#event, #call)
        }
    }
}
