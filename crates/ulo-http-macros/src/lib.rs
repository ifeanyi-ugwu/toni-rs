//! The HTTP handler attributes, re-exported by `ulo-http`: `#[get]`, `#[post]`, `#[put]`,
//! `#[patch]`, `#[delete]`, `#[head]`, `#[options]`. Each is a thin layer over
//! `ulo-handler-codegen`, and each goes on a method of a `#[routes]` impl:
//!
//! ```ignore
//! #[routes]
//! impl UsersController {
//!     #[ulo_http::get("/users/{id}")]
//!     async fn get(&self, id: Path<u64>) -> Result<Json<User>, UserError> { /* .. */ }
//! }
//! ```
//!
//! The argument is the route pattern: `{name}` segments plus an optional trailing `{*rest}`. A
//! malformed literal, `/u/{id`, is a span error on it.

mod pattern;
mod route;

use proc_macro::TokenStream;

/// A `GET` route. A `HEAD` request on its path is answered from it with the body omitted, unless
/// a `#[head]` route exists.
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("GET", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("POST", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("PUT", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_attribute]
pub fn patch(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("PATCH", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("DELETE", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_attribute]
pub fn head(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("HEAD", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// An `OPTIONS` route. Without one, `OPTIONS` on a matching path answers 204 with `Allow`; a CORS
/// preflight never reaches routing.
#[proc_macro_attribute]
pub fn options(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::expand("OPTIONS", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
