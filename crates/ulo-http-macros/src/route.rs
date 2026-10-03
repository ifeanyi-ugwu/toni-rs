//! One HTTP handler attribute over `ulo-handler-codegen`.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, and the three items the
//! `__handler` protocol owes (`ulo_handler_codegen::emit::MountFn`), the handler value being
//!
//! ```text
//! ::ulo_http::__private::HttpHandler::new(::ulo_http::__private::Method::GET, "/users/{id}", __ulo_call)
//!     .path_check((&&::ulo_http::__private::PathProbe::<P0>::new()).check())
//!     /* one per parameter */
//! ```
//!
//! with `ViaPath` and `NotPath` imported anonymously, and the route `"/users/{id}"` as
//! `HandlerSpec::route`.

use proc_macro2::TokenStream;
use syn::{ImplItemFn, LitStr};
use ulo_handler_codegen::Paths;

/// The transport's key, which equals `<ulo_http::Http as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "http";

/// `#[<method>("<pattern>")]` on `item`. `method` is the HTTP method's name as `http::Method`'s
/// associated constant spells it.
pub(crate) fn expand(method: &'static str, attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let pattern: LitStr = syn::parse2(attr)?;
    if let Err(reason) = crate::pattern::check(&pattern.value()) {
        return Err(syn::Error::new(pattern.span(), format!("invalid route pattern: {reason}")));
    }
    let mut item: ImplItemFn = syn::parse2(item)?;
    let paths = Paths::new("ulo_http", "Http");
    let _ = (method, &mut item, &paths, KEY);
    todo!("take `__handler` (absent: an error saying the attribute goes in a `#[routes]` impl), analyze, rewrite returns, build the handler value, emit `MountFn`")
}
