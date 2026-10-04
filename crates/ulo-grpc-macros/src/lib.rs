//! The gRPC attribute, re-exported by `ulo-grpc`: `#[method(Marker)]` on a method of a `#[routes]`
//! impl, a thin layer over `ulo-handler-codegen`.
//!
//! ```ignore
//! #[routes]
//! impl UsersGrpc {
//!     #[ulo_grpc::method(pb::user_service::GetUser)]
//!     async fn get_user(&self, req: Message<pb::GetUserRequest>) -> Result<Response<pb::User>, UserError> { /* .. */ }
//! }
//! ```
//!
//! The marker's `Request`, `Response` and `SHAPE` are checked against the handler's signature
//! through trait bounds at the concrete site, so a mismatch fails to compile.

mod method;

use proc_macro::TokenStream;

/// A handler bound to the generated method marker `Marker`.
#[proc_macro_attribute]
pub fn method(attr: TokenStream, item: TokenStream) -> TokenStream {
    method::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
